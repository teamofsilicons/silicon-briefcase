//! Postmark invitation delivery. Credentials and recipient addresses never enter logs.

use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use sqlx::PgPool;

pub(super) struct Sender {
    client: reqwest::Client,
    token: Option<SecretString>,
    site: url::Url,
    endpoint: url::Url,
}

impl Sender {
    pub(super) fn from_environment() -> anyhow::Result<Self> {
        let token = std::env::var("BRIEFCASE_POSTMARK_SERVER_TOKEN")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(SecretString::from);
        let site = url::Url::parse(
            &std::env::var("BRIEFCASE_PUBLIC_SITE_BASE_URL")
                .unwrap_or_else(|_| "https://briefcase.teamofsilicons.com/".to_owned()),
        )?;
        anyhow::ensure!(
            matches!(site.scheme(), "http" | "https"),
            "invalid public site URL"
        );
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            token,
            site,
            endpoint: url::Url::parse("https://api.postmarkapp.com/email")?,
        })
    }

    pub(super) async fn send(
        &self,
        pool: &PgPool,
        org: &str,
        id: uuid::Uuid,
        payload: &Value,
        test: bool,
    ) -> Result<(), &'static str> {
        // Test environments exercise the outbox without contacting real inboxes.
        if test {
            return Ok(());
        }
        let token = self.token.as_ref().ok_or("postmark_unconfigured")?;
        let recipient_type = payload["recipient_type"]
            .as_str()
            .ok_or("invitation_payload_invalid")?;
        let recipient_id = payload["recipient_id"]
            .as_str()
            .ok_or("invitation_payload_invalid")?;
        let email:Option<String>=sqlx::query_scalar("SELECT c.email FROM briefcase.member_contacts c JOIN briefcase.organization_members m USING(org_id,actor_type,actor_id) WHERE c.org_id=$1 AND c.actor_type=$2 AND c.actor_id=$3 AND m.membership_status='active'")
            .bind(org).bind(recipient_type).bind(recipient_id).fetch_optional(pool).await.map_err(|_|"invitation_contact_lookup_failed")?;
        let email = email.ok_or("invitation_contact_unavailable")?;
        let body = self.message(org, id, payload, &email)?;
        self.deliver(&body, token).await
    }

    fn message(
        &self,
        org: &str,
        id: uuid::Uuid,
        payload: &Value,
        email: &str,
    ) -> Result<Value, &'static str> {
        let mut link = self.site.clone();
        let path = payload["details"]["path"]
            .as_str()
            .ok_or("invitation_payload_invalid")?;
        {
            let mut segments = link
                .path_segments_mut()
                .map_err(|()| "invitation_url_invalid")?;
            segments.pop_if_empty().extend(["org", org]);
            segments.extend(path.split('/'));
            segments.push("");
        }
        let name = payload["details"]["name"]
            .as_str()
            .unwrap_or("a file or folder");
        let body = json!({"From":"briefcase@teamofsilicons.com","To":email,"Subject":"You've been invited to a Briefcase file or folder",
            "TextBody":format!("You have been invited to {name} in {org}.\n\nOpen Briefcase to view your current permissions:\n{link}\n"),
            "MessageStream":"outbound","TrackOpens":false,"TrackLinks":"None","Metadata":{"invitation_id":id.to_string()}});
        Ok(body)
    }

    async fn deliver(&self, body: &Value, token: &SecretString) -> Result<(), &'static str> {
        let mut response = self
            .client
            .post(self.endpoint.clone())
            .header("X-Postmark-Server-Token", token.expose_secret())
            .header("Accept", "application/json")
            .json(body)
            .send()
            .await
            .map_err(|_| "postmark_unavailable")?;
        if !response.status().is_success() {
            return Err("postmark_rejected");
        }
        if response.content_length().is_some_and(|size| size > 16_384) {
            return Err("postmark_invalid_response");
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "postmark_invalid_response")?
        {
            if bytes.len() + chunk.len() > 16_384 {
                return Err("postmark_invalid_response");
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| "postmark_invalid_response")?;
        if value["ErrorCode"].as_u64() != Some(0) {
            return Err("postmark_rejected");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path},
    };

    #[tokio::test]
    async fn invitation_mail_uses_verified_recipient_and_rejects_provider_failures()
    -> anyhow::Result<()> {
        let server = MockServer::start().await;
        let sender = Sender {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            token: Some(SecretString::from("test-token")),
            site: url::Url::parse("https://briefcase.teamofsilicons.com/")?,
            endpoint: url::Url::parse(&format!("{}/email", server.uri()))?,
        };
        let id = uuid::Uuid::new_v4();
        let body = sender
            .message(
                "tos",
                id,
                &json!({"details":{"path":"private/me:tos/a report.md","name":"A report"}}),
                "verified@example.com",
            )
            .map_err(anyhow::Error::msg)?;
        assert_eq!(body["From"], "briefcase@teamofsilicons.com");
        assert_eq!(body["To"], "verified@example.com");
        assert_eq!(body["TrackOpens"], false);
        assert!(
            body["TextBody"]
                .as_str()
                .is_some_and(|text| text.contains("/org/tos/private/me:tos/a%20report.md/"))
        );
        for (status, response, expected) in [
            (200, json!({"ErrorCode":0}), Ok(())),
            (200, json!({"ErrorCode":300}), Err("postmark_rejected")),
            (503, json!({}), Err("postmark_rejected")),
        ] {
            server.reset().await;
            Mock::given(method("POST"))
                .and(path("/email"))
                .and(header("X-Postmark-Server-Token", "test-token"))
                .respond_with(ResponseTemplate::new(status).set_body_json(response))
                .expect(1)
                .mount(&server)
                .await;
            let result = sender
                .deliver(&body, &SecretString::from("test-token"))
                .await;
            assert_eq!(result, expected);
            server.verify().await;
        }
        server.reset().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(16385)))
            .mount(&server)
            .await;
        assert_eq!(
            sender
                .deliver(&body, &SecretString::from("test-token"))
                .await,
            Err("postmark_invalid_response")
        );
        Ok(())
    }
}
