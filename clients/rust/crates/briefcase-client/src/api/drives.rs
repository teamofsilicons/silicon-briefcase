//! The signed-in account, its drives, what others shared with it, who may
//! share with a Silicon, and each drive's usage and storage.

use reqwest::Method;

use crate::{
    client::{Client, IdempotencyKey, json_body},
    error::Result,
    models::{
        Account, AccountsInfo, AllowedAccounts, BucketConfiguration, BucketConfigurationStatus,
        DriveList, DriveUsage, SharedPage,
    },
};

impl Client {
    /// Reads how this deployment signs accounts in: its app id at Silicon
    /// Accounts and the Silicon Accounts URL (`GET /api/v1/accounts`). Needs no
    /// credential; check it before spending a single-use sign-in token.
    ///
    /// # Errors
    ///
    /// Returns an error when the deployment cannot be reached.
    pub async fn accounts_info(&self) -> Result<AccountsInfo> {
        let request = self
            .anonymous_request(Method::GET, self.api_url(&["accounts"])?)
            .timeout(self.request_timeout());
        self.receive_json(request).await
    }

    /// Reads the signed-in account as Briefcase verified it (`GET /api/v1/me`):
    /// uuid, id, kind, display name, photo, and a Silicon's custodian.
    ///
    /// # Errors
    ///
    /// Returns an unauthenticated error (`token_expired`, `token_revoked`,
    /// `account_deleted`…) when the access token is not accepted.
    pub async fn me(&self) -> Result<Account> {
        let request = self
            .request(Method::GET, self.api_url(&["me"])?)
            .timeout(self.request_timeout());
        self.receive_json(request).await
    }

    /// Lists the drives the caller opens with the owner's rights: its own and,
    /// for a Carbon, one per Silicon it looks after.
    ///
    /// # Errors
    ///
    /// Returns an error when the deployment cannot be reached.
    pub async fn drives(&self) -> Result<DriveList> {
        let request = self
            .request(Method::GET, self.api_url(&["drives"])?)
            .timeout(self.request_timeout());
        self.receive_json(request).await
    }

    /// Lists what other accounts shared with the caller (Shared with me),
    /// newest share first, one item per entry with its drive's owner.
    ///
    /// # Errors
    ///
    /// Returns an error when the cursor is not one Briefcase issued.
    pub async fn shared(&self, cursor: Option<&str>, limit: Option<u16>) -> Result<SharedPage> {
        let mut url = self.api_url(&["shared"])?;
        {
            let mut pairs = url.query_pairs_mut();
            if let Some(cursor) = cursor {
                pairs.append_pair("cursor", cursor);
            }
            if let Some(limit) = limit {
                pairs.append_pair("limit", &limit.to_string());
            }
        }
        let request = self
            .request(Method::GET, url)
            .timeout(self.request_timeout());
        self.receive_json(request).await
    }

    /// Lists the accounts a Silicon allows to share with it besides its
    /// custodian and the custodian's other Silicons. Only the Silicon itself
    /// and its custodian may read it.
    ///
    /// # Errors
    ///
    /// Returns `not_silicons_custodian` (403) for anyone else and `not_a_silicon`
    /// when `silicon` names a Carbon.
    pub async fn allowed_accounts(&self, silicon: &str) -> Result<AllowedAccounts> {
        let url = self.api_url(&["silicons", silicon, "allowed-accounts"])?;
        let request = self
            .request(Method::GET, url)
            .timeout(self.request_timeout());
        self.receive_json(request).await
    }

    /// Allows an account (by `c:`/`si:` id or uuid) to share with a Silicon.
    /// Only the Silicon itself and its custodian may change the list.
    ///
    /// # Errors
    ///
    /// Returns `not_silicons_custodian`, `not_a_silicon`, `allowance_self`, or
    /// `unknown_account`.
    pub async fn allow_account(&self, silicon: &str, account: &str) -> Result<()> {
        let url = self.api_url(&["silicons", silicon, "allowed-accounts", account])?;
        let request = self
            .request(Method::PUT, url)
            .timeout(self.request_timeout());
        self.receive_empty(request).await
    }

    /// Stops allowing an account to share with a Silicon. Shares it already
    /// made stay until they are revoked one by one.
    ///
    /// # Errors
    ///
    /// The same as [`Client::allow_account`].
    pub async fn disallow_account(&self, silicon: &str, account: &str) -> Result<()> {
        let url = self.api_url(&["silicons", silicon, "allowed-accounts", account])?;
        let request = self
            .request(Method::DELETE, url)
            .timeout(self.request_timeout());
        self.receive_empty(request).await
    }

    /// Reports what a drive is consuming, in exact bytes (the caller's own
    /// when `drive` is absent). Needs the drive owner's rights: its owner, or
    /// the custodian of the Silicon that owns it.
    ///
    /// Storage counts every retained version, binned entries included, because
    /// those bytes are still stored; the space returns when the bin is purged.
    ///
    /// # Errors
    ///
    /// Returns a not-found error for a drive the caller does not own.
    pub async fn usage(&self, drive: Option<&str>) -> Result<DriveUsage> {
        let mut url = self.api_url(&["usage"])?;
        if let Some(drive) = drive {
            url.query_pairs_mut().append_pair("drive", drive);
        }
        let request = self
            .request(Method::GET, url)
            .timeout(self.request_timeout());
        self.receive_json(request).await
    }

    /// Points a drive's new files at a bucket its owner brings
    /// ([`BucketConfiguration::drive`]; the caller's own by default).
    ///
    /// Briefcase assumes the role and performs a create, read, update, and
    /// delete probe; the bucket becomes active only when every check passes,
    /// and a failed probe leaves the previous configuration in place.
    ///
    /// # Errors
    ///
    /// Returns a forbidden error unless the caller owns the drive or is the
    /// custodian of the Silicon that does.
    pub async fn configure_storage(
        &self,
        configuration: &BucketConfiguration,
    ) -> Result<BucketConfigurationStatus> {
        self.configure_storage_with_key(configuration, &IdempotencyKey::random())
            .await
    }

    /// Configures storage with a caller-owned retry identity.
    ///
    /// Retain this key together with the exact configuration before sending.
    /// Reuse both after a lost response to recover the same validation outcome.
    /// To retry a completed failed probe after fixing the external bucket or
    /// role policy, use a new key: the old key replays that failed result.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Client::configure_storage`], or a conflict
    /// when the key was previously used for a different configuration.
    pub async fn configure_storage_with_key(
        &self,
        configuration: &BucketConfiguration,
        idempotency_key: &IdempotencyKey,
    ) -> Result<BucketConfigurationStatus> {
        let url = self.api_url(&["storage", "configuration"])?;
        let body = json_body(configuration)?;
        let request = self
            .request(Method::PUT, url)
            .header("content-type", "application/json")
            .header("idempotency-key", idempotency_key.as_str())
            .body(body)
            .timeout(self.transfer_timeout());
        self.receive_json(request).await
    }
}
