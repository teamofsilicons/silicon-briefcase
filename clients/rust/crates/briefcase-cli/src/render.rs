//! Turning answers into something worth reading.
//!
//! Two modes, chosen once: a table meant for a person, and `--json` meant for
//! whatever comes next in the pipe. The JSON is the contract's own shape, not
//! a reformatting of it, so a script never has to parse a table.

use std::io::Write as _;

use briefcase_client::{
    ActivityEvent, Entry, EntryPage, EntryType, EntryVisibility, FileVersion, Invitation,
    InvitationPage, LinkAccess, Notification, NotificationInbox, OrganizationUsage,
    PermissionInspection, Recipient, SearchResult,
};
use serde::Serialize;
use time::{
    OffsetDateTime, UtcOffset,
    format_description::{BorrowedFormatItem, well_known::Rfc3339},
    macros::format_description,
};

const TIMESTAMP: &[BorrowedFormatItem<'_>] =
    format_description!("[year]-[month]-[day] [hour]:[minute]");

/// How results are printed.
#[derive(Clone, Copy, Debug)]
pub struct Output {
    json: bool,
}

impl Output {
    /// Chooses the output mode for this run.
    #[must_use]
    pub const fn new(json: bool) -> Self {
        Self { json }
    }

    /// Returns whether machine-readable output was asked for.
    #[must_use]
    pub const fn is_json(self) -> bool {
        self.json
    }

    /// Prints a value as JSON, for `--json`.
    pub fn json<T: Serialize>(self, value: &T) {
        let _ = self;
        print_json(value);
    }

    /// Prints a one-line confirmation, unless JSON was asked for.
    pub fn note(self, message: &str) {
        if !self.json {
            println!("{}", terminal_text(message));
        }
    }

    /// Prints a listing of entries.
    pub fn entries(self, entries: &[Entry], long: bool) {
        if self.json {
            self.json(&entries);
            return;
        }
        if entries.is_empty() {
            println!("(nothing here)");
            return;
        }
        // The column appears only when something listed will self-destruct, so
        // a file about to vanish is never mistaken for an ordinary one.
        let self_destructing = entries.iter().any(|entry| entry.self_destruct_at.is_some());
        let now = OffsetDateTime::now_utc();
        let mut rows = Vec::with_capacity(entries.len());
        for entry in entries {
            let mut row = vec![
                kind_marker(entry).to_owned(),
                entry.name.clone(),
                entry.size.map_or_else(|| "-".to_owned(), human_size),
                entry.updated_at.map_or_else(|| "-".to_owned(), timestamp),
            ];
            if self_destructing {
                row.push(
                    entry
                        .self_destruct_at
                        .map_or_else(|| "-".to_owned(), |moment| relative(moment, now)),
                );
            }
            if long {
                row.push(
                    entry
                        .owner
                        .as_ref()
                        .map_or_else(|| "-".to_owned(), ToString::to_string),
                );
                row.push(access_summary(entry));
                row.push(entry.path.clone());
            }
            rows.push(row);
        }
        let mut headers = vec!["", "NAME", "SIZE", "UPDATED"];
        if self_destructing {
            headers.push("SELF-DESTRUCTS");
        }
        if long {
            headers.extend(["OWNER", "ACCESS", "PATH"]);
        }
        print_table(&headers, &rows);
    }

    /// Prints one entry page and preserves its continuation cursor.
    pub fn entry_page(self, page: &EntryPage, long: bool) {
        if self.json {
            self.json(page);
            return;
        }
        self.entries(&page.items, long);
        if let Some(cursor) = &page.next_cursor {
            eprintln!(
                "(more entries remain; continue with --cursor {cursor}, or pass --all to follow every page)"
            );
        }
    }

    /// Prints one entry in full.
    pub fn entry(self, entry: &Entry) {
        if self.json {
            self.json(entry);
            return;
        }
        let mut fields = vec![
            ("path", entry.path.clone()),
            (
                "type",
                match entry.entry_type {
                    EntryType::File => "file".to_owned(),
                    EntryType::Folder => "folder".to_owned(),
                },
            ),
            ("id", entry.id.to_string()),
            ("boundary", format!("{:?}", entry.root_type).to_lowercase()),
        ];
        if let Some(tag) = &entry.tag {
            fields.push(("tag", tag.clone()));
        }
        if let Some(size) = entry.size {
            fields.push(("size", format!("{} ({size} bytes)", human_size(size))));
        }
        if let Some(content_type) = &entry.content_type {
            fields.push(("media type", content_type.clone()));
        }
        if let Some(render) = entry.render {
            fields.push(("renderer", format!("{render:?}").to_lowercase()));
        }
        if let Some(owner) = &entry.owner {
            fields.push(("owner", owner.to_string()));
        }
        if let Some(app) = &entry.origin_app_id {
            fields.push(("created by app", app.clone()));
        }
        if entry.visibility == EntryVisibility::Traversal {
            fields.push((
                "visibility",
                "traversal (reachable because something inside was shared)".to_owned(),
            ));
        }
        fields.push(("you may", access_summary(entry)));
        if let Some(created) = entry.created_at {
            fields.push(("created", timestamp(created)));
        }
        if let Some(updated) = entry.updated_at {
            fields.push(("updated", timestamp(updated)));
        }
        if let Some(deleted) = entry.deleted_at {
            fields.push(("in the bin since", timestamp(deleted)));
        }
        if let Some(moment) = entry.self_destruct_at {
            fields.push((
                "self-destructs",
                format!(
                    "{}; deleted for good, never to the bin",
                    deadline(moment, OffsetDateTime::now_utc())
                ),
            ));
        }
        fields.push(("url", entry.permanent_url.to_string()));

        let width = fields
            .iter()
            .map(|(label, _)| label.len())
            .max()
            .unwrap_or(0);
        for (label, value) in fields {
            println!("{label:<width$}  {}", terminal_text(&value));
        }
    }

    /// Prints search results with why each one matched.
    pub fn search(self, results: &[SearchResult]) {
        if self.json {
            self.json(&results);
            return;
        }
        if results.is_empty() {
            println!("(no matches)");
            return;
        }
        let rows: Vec<Vec<String>> = results
            .iter()
            .map(|result| {
                vec![
                    result.entry.name.clone(),
                    if result.filename_match {
                        "name".to_owned()
                    } else {
                        format!("{} in text", result.content_hits)
                    },
                    result.entry.path.clone(),
                ]
            })
            .collect();
        print_table(&["NAME", "MATCHED", "PATH"], &rows);
        for result in results {
            for snippet in &result.snippets {
                // ts_headline supplies HTML highlight markers. Human terminal
                // output is plain text; machine JSON retains the wire value.
                let excerpt = snippet
                    .replace("<b>", "")
                    .replace("</b>", "")
                    .replace('\n', " ");
                println!(
                    "    {}: {}",
                    terminal_text(&result.entry.name),
                    terminal_text(&excerpt)
                );
            }
        }
    }

    /// Prints what the caller may do on a batch of targets.
    pub fn inspection(self, inspection: &PermissionInspection) {
        if self.json {
            self.json(inspection);
            return;
        }
        let rows: Vec<Vec<String>> = inspection
            .items
            .iter()
            .map(|item| {
                vec![
                    item.path.clone(),
                    item.effective_access
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(","),
                ]
            })
            .collect();
        if !rows.is_empty() {
            print_table(&["PATH", "YOU MAY"], &rows);
        }
        for path in &inspection.unresolved_paths {
            println!("{}: not found, or not yours to see", terminal_text(path));
        }
        for id in &inspection.unresolved_entry_ids {
            println!("{id}: not found, or not yours to see");
        }
    }

    /// Prints a file's retained versions.
    pub fn versions(self, versions: &[FileVersion]) {
        if self.json {
            self.json(&versions);
            return;
        }
        if versions.is_empty() {
            println!("(no retained versions)");
            return;
        }
        let rows: Vec<Vec<String>> = versions
            .iter()
            .map(|version| {
                vec![
                    version.number.to_string(),
                    human_size(version.size),
                    version.created_by.to_string(),
                    timestamp(version.created_at),
                    version.id.to_string(),
                ]
            })
            .collect();
        print_table(&["#", "SIZE", "AUTHOR", "WHEN", "VERSION"], &rows);
    }

    /// Prints an entry's recorded history.
    pub fn history(self, events: &[ActivityEvent]) {
        if self.json {
            self.json(&events);
            return;
        }
        if events.is_empty() {
            println!("(no recorded history)");
            return;
        }
        let rows: Vec<Vec<String>> = events
            .iter()
            .map(|event| {
                vec![
                    timestamp(event.occurred_at),
                    event.action.clone(),
                    event.actor.to_string(),
                    event.app_id.clone().unwrap_or_else(|| "-".to_owned()),
                ]
            })
            .collect();
        print_table(&["WHEN", "ACTION", "ACTOR", "APP"], &rows);
    }

    /// Prints the notification inbox.
    pub fn inbox(self, inbox: &NotificationInbox) {
        if self.json {
            self.json(inbox);
            return;
        }
        println!(
            "{} unread of {} shown",
            inbox.unread_count,
            inbox.items.len()
        );
        if inbox.items.is_empty() {
            return;
        }
        let rows: Vec<Vec<String>> = inbox.items.iter().map(notification_row).collect();
        print_table(&["", "WHEN", "WHAT", "WHO", "ENTRY"], &rows);
    }

    /// Prints what the organization is consuming.
    pub fn usage(self, usage: &OrganizationUsage) {
        if self.json {
            self.json(usage);
            return;
        }
        println!(
            "storage        {} of {} used, {} left",
            human_size(usage.storage.used_bytes),
            human_size(usage.storage.limit_bytes),
            human_size(usage.storage.remaining_bytes)
        );
        println!(
            "uploads today  {} of {} used, {} left, resets {}",
            human_size(usage.daily_uploads.used_bytes),
            human_size(usage.daily_uploads.limit_bytes),
            human_size(usage.daily_uploads.remaining_bytes),
            timestamp(usage.daily_uploads.resets_at)
        );
    }

    /// Prints one invitation as JSON, then says on stderr when an expiring share
    /// ends, unless JSON was asked for.
    ///
    /// Standard output stays the service's JSON either way, so a script that
    /// never passed `--json` keeps parsing it.
    pub fn invitation(self, invitation: &Invitation) {
        self.json(invitation);
        self.expiring_note(invitation);
    }

    /// Prints a page of invitations as JSON, then notes each expiring share's end.
    pub fn invitations(self, page: &InvitationPage) {
        self.json(page);
        for invitation in &page.items {
            self.expiring_note(invitation);
        }
    }

    /// Prints a link setting as JSON, then notes when an expiring link ends.
    pub fn link(self, link: &LinkAccess) {
        self.json(link);
        if let Some(expires_at) = &link.expires_at {
            self.aside(&format!(
                "expiring link ends {}",
                rfc3339_deadline(expires_at)
            ));
        }
    }

    fn expiring_note(self, invitation: &Invitation) {
        if let Some(expires_at) = &invitation.expires_at {
            self.aside(&format!(
                "expiring share {} for {} ends {}",
                invitation.id,
                recipient(&invitation.invitation.principal),
                rfc3339_deadline(expires_at)
            ));
        }
    }

    /// Prints a person-only remark to stderr, leaving stdout for the answer.
    pub fn aside(self, message: &str) {
        if !self.json {
            eprintln!("{}", terminal_text(message));
        }
    }

    /// Writes raw bytes to standard output, for `cat`.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the pipe is closed.
    pub fn bytes(self, bytes: &[u8]) -> std::io::Result<()> {
        let _ = self;
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(bytes)?;
        stdout.flush()
    }
}

fn notification_row(notification: &Notification) -> Vec<String> {
    let what = match notification.kind {
        briefcase_client::NotificationKind::AccessGranted => "access granted",
        briefcase_client::NotificationKind::AccessRevoked => "access revoked",
        briefcase_client::NotificationKind::AccessRequested => "access requested",
        briefcase_client::NotificationKind::AccessRequestDecided => "request decided",
    };
    let detail = notification.decision.map_or_else(
        || what.to_owned(),
        |decision| format!("{what} ({decision:?})").to_lowercase(),
    );
    vec![
        if notification.read { " " } else { "*" }.to_owned(),
        timestamp(notification.created_at),
        detail,
        notification
            .actor
            .as_ref()
            .map_or_else(|| "-".to_owned(), ToString::to_string),
        notification
            .subject
            .as_ref()
            .map_or_else(|| "-".to_owned(), |subject| subject.path.clone()),
    ]
}

fn recipient(principal: &Recipient) -> String {
    match principal {
        Recipient::Carbon(id) | Recipient::Silicon(id) => id.clone(),
        Recipient::Email(address) => format!("email:{address}"),
        Recipient::Tag(tag) => format!("tag:{tag}"),
    }
}

fn kind_marker(entry: &Entry) -> &'static str {
    match entry.entry_type {
        EntryType::Folder => "d",
        EntryType::File => "-",
    }
}

fn access_summary(entry: &Entry) -> String {
    if entry.effective_access.is_empty() {
        return "-".to_owned();
    }
    entry
        .effective_access
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// Formats a byte count the way a person reads one.
#[must_use]
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    #[allow(clippy::cast_precision_loss)]
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    // A figure that rounds to 1024 of its unit belongs in the next one: a
    // 1 PiB ceiling should read as 1.0 PiB, not 1024 TiB.
    if value >= 1023.95 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Formats a timestamp in UTC, to the minute.
#[must_use]
pub fn timestamp(moment: OffsetDateTime) -> String {
    moment
        .format(TIMESTAMP)
        .unwrap_or_else(|_| moment.unix_timestamp().to_string())
}

/// Formats a deadline as its UTC minute and how far away it is, such as
/// `2026-09-24 18:05 UTC (in 2h 5m)`.
#[must_use]
pub fn deadline(moment: OffsetDateTime, now: OffsetDateTime) -> String {
    format!(
        "{} UTC ({})",
        timestamp(moment.to_offset(UtcOffset::UTC)),
        relative(moment, now)
    )
}

/// Formats a wire RFC 3339 deadline, or returns it untouched when unreadable.
fn rfc3339_deadline(value: &str) -> String {
    OffsetDateTime::parse(value, &Rfc3339).map_or_else(
        |_| value.to_owned(),
        |moment| deadline(moment, OffsetDateTime::now_utc()),
    )
}

/// Says how far away a deadline is, such as `in 2h 5m`, rounding up to the
/// minute so a deadline never reads as sooner than it is.
#[must_use]
pub fn relative(moment: OffsetDateTime, now: OffsetDateTime) -> String {
    let seconds = (moment - now).whole_seconds();
    if seconds <= 0 {
        return "due now".to_owned();
    }
    let minutes = u64::try_from(seconds).unwrap_or(u64::MAX).div_ceil(60);
    format!("in {}", span(minutes))
}

/// Formats a whole number of minutes in days, hours and minutes, such as
/// `1d 12h` or `2h 5m`.
#[must_use]
pub fn span(minutes: u64) -> String {
    let parts: Vec<String> = [
        (minutes / (24 * 60), "d"),
        (minutes % (24 * 60) / 60, "h"),
        (minutes % 60, "m"),
    ]
    .into_iter()
    .filter(|(amount, _)| *amount > 0)
    .map(|(amount, unit)| format!("{amount}{unit}"))
    .collect();
    if parts.is_empty() {
        "0m".to_owned()
    } else {
        parts.join(" ")
    }
}

/// Prints a value as JSON.
fn print_json<T: Serialize>(value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(rendered) => println!("{rendered}"),
        Err(error) => eprintln!("briefcase: the answer could not be rendered: {error}"),
    }
}

fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| row.iter().map(|cell| terminal_text(cell)).collect())
        .collect();
    let columns = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|header| header.len()).collect();
    for row in &rows {
        for (index, cell) in row.iter().enumerate().take(columns) {
            widths[index] = widths[index].max(cell.chars().count());
        }
    }
    let mut line = String::new();
    for (index, (header, width)) in headers.iter().zip(&widths).enumerate() {
        push_cell(&mut line, header, *width, index + 1 == columns);
    }
    println!("{}", line.trim_end());
    for row in &rows {
        let mut line = String::new();
        for (index, width) in widths.iter().enumerate() {
            let cell = row.get(index).map_or("", String::as_str);
            push_cell(&mut line, cell, *width, index + 1 == columns);
        }
        println!("{}", line.trim_end());
    }
}

// Names, paths and excerpts are user-controlled. Never interpret their terminal
// control sequences or bidi overrides as display instructions. Raw file output
// (`cat`/download) and structured JSON deliberately preserve the original data.
pub(crate) fn terminal_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control()
            || matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            output.extend(character.escape_default());
        } else {
            output.push(character);
        }
    }
    output
}

fn push_cell(line: &mut String, cell: &str, width: usize, last: bool) {
    line.push_str(cell);
    if !last {
        let padding = width.saturating_sub(cell.chars().count()) + 2;
        line.push_str(&" ".repeat(padding));
    }
}

#[cfg(test)]
mod tests {
    use super::{deadline, human_size, relative, span};
    use time::macros::datetime;

    #[test]
    fn spans_read_in_days_hours_and_minutes() {
        assert_eq!(span(0), "0m");
        assert_eq!(span(1), "1m");
        assert_eq!(span(90), "1h 30m");
        assert_eq!(span(120), "2h");
        assert_eq!(span(2_160), "1d 12h");
        assert_eq!(span(43_200), "30d");
        assert_eq!(span(43_199), "29d 23h 59m");
    }

    #[test]
    fn deadlines_say_when_and_how_far_away_in_utc() {
        let now = datetime!(2026-09-24 16:00:00 UTC);
        assert_eq!(
            relative(datetime!(2026-09-24 18:05:00 UTC), now),
            "in 2h 5m"
        );
        // A partial minute rounds up, never down to "sooner than it is".
        assert_eq!(relative(datetime!(2026-09-24 16:00:01 UTC), now), "in 1m");
        assert_eq!(relative(now, now), "due now");
        assert_eq!(relative(datetime!(2026-09-24 15:00:00 UTC), now), "due now");
        assert_eq!(
            deadline(datetime!(2026-09-24 20:05:00 +02:00), now),
            "2026-09-24 18:05 UTC (in 2h 5m)"
        );
    }

    #[test]
    fn sizes_read_the_way_people_write_them() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(1024), "1.0 KiB");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(1024 * 1024), "1.0 MiB");
        assert_eq!(human_size(150 * 1024 * 1024), "150 MiB");
        assert_eq!(human_size(1 << 50), "1.0 PiB");
        // Just under a petabyte still reads as a petabyte, not 1024 TiB.
        assert_eq!(human_size((1 << 50) - 184), "1.0 PiB");
    }
}
