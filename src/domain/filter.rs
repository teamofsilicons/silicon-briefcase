//! The entry filter language.
//!
//! The product contract asks for filters that combine freely: any predicate
//! with any other, in any permutation. That is a small query language rather
//! than a fixed set of query parameters, so it is parsed here — in the domain,
//! with no knowledge of SQL — into an expression tree that persistence
//! compiles and policy can post-filter.
//!
//! ```text
//! last:5 after:02-06-2026 location:'private/*' (contains:'apple' or contains:'cat') is:md
//! ```
//!
//! Terms combine with `and` unless separated by `or`. `not` and a leading `-`
//! negate.
//! Parentheses group. `last:`, `first:`, and `sort:` are result modifiers and
//! may appear only at the top level, because reordering half of a boolean
//! expression is meaningless.

use std::fmt;

use thiserror::Error;
use time::{Date, macros::format_description};

use super::{
    actor::{ActorKind, ActorRef},
    entry::EntryKind,
    media::RenderKind,
    permission::EffectiveAccess,
};

/// Largest number of results `last:` or `first:` may take.
pub const MAX_FILTER_TAKE: u16 = 100;
/// Largest accepted filter expression, in bytes.
pub const MAX_FILTER_LENGTH: usize = 1_024;
/// Largest number of predicates one expression may contain.
pub const MAX_FILTER_PREDICATES: usize = 32;

const DATE_FORMAT: &[time::format_description::FormatItem<'static>] =
    format_description!("[day]-[month]-[year]");

/// Which chronological end a `last:` or `first:` modifier takes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TakeEnd {
    /// The newest N entries.
    Newest,
    /// The oldest N entries.
    Oldest,
}

/// A `last:N` or `first:N` result cap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Take {
    /// Which end of the timeline the entries come from.
    pub end: TakeEnd,
    /// How many entries to take.
    pub count: u16,
}

/// Presentation order of a filtered listing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SortOrder {
    /// Newest first. This is the default, so the oldest entry is last.
    #[default]
    Newest,
    /// Oldest first.
    Oldest,
}

/// A text term whose `*` characters are wildcards.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlobTerm(String);

impl GlobTerm {
    /// Returns the raw term, wildcards included.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns whether the term contains a wildcard.
    #[must_use]
    pub fn has_wildcard(&self) -> bool {
        self.0.contains('*')
    }

    /// Renders the term as a SQL `LIKE` pattern.
    ///
    /// A term without wildcards matches anywhere in the value, which is what
    /// "contains" means to a person typing a filter.
    #[must_use]
    pub fn like_pattern(&self) -> String {
        let mut pattern = String::with_capacity(self.0.len() + 2);
        if !self.0.starts_with('*') {
            pattern.push('%');
        }
        for character in self.0.chars() {
            match character {
                '*' => pattern.push('%'),
                '%' | '_' | '\\' => {
                    pattern.push('\\');
                    pattern.push(character);
                }
                other => pattern.push(other),
            }
        }
        if !self.0.ends_with('*') {
            pattern.push('%');
        }
        pattern
    }

    /// Renders the term as a SQL `LIKE` pattern anchored at the start.
    ///
    /// Locations are prefixes: `location:'private'` selects the Private tree.
    #[must_use]
    pub fn prefix_pattern(&self) -> String {
        let mut pattern = String::with_capacity(self.0.len() + 1);
        for character in self.0.trim_matches('/').chars() {
            match character {
                '*' => pattern.push('%'),
                '%' | '_' | '\\' => {
                    pattern.push('\\');
                    pattern.push(character);
                }
                other => pattern.push(other),
            }
        }
        if !pattern.ends_with('%') {
            pattern.push('%');
        }
        pattern
    }
}

/// A member named by a `from:`, `to:`, or `for:` filter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActorSelector {
    /// Required principal kind, when the filter named one.
    pub kind: Option<ActorKind>,
    /// IAM principal identifier.
    pub id: String,
}

impl ActorSelector {
    /// Returns whether an actor satisfies this selector.
    #[must_use]
    pub fn matches(&self, actor: &ActorRef) -> bool {
        self.kind.is_none_or(|kind| kind == actor.kind()) && actor.id().as_str() == self.id
    }
}

/// One filter condition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FilterPredicate {
    /// Changed on or after the given day, inclusive.
    ChangedAfter(Date),
    /// Changed strictly before the given day.
    ChangedBefore(Date),
    /// Changed within an inclusive day range.
    ChangedBetween(Date, Date),
    /// Created by the named member.
    CreatedBy(ActorSelector),
    /// Explicitly shared with the named member.
    SharedWith(ActorSelector),
    /// Reachable by the named member.
    AccessibleTo(ActorSelector),
    /// Name or extracted content matches the term.
    Contains(GlobTerm),
    /// Extracted content matches the term.
    HasContent(GlobTerm),
    /// Name matches the term.
    NameMatches(GlobTerm),
    /// Is a file or a folder.
    IsKind(EntryKind),
    /// Opens with the given renderer.
    IsRender(RenderKind),
    /// Has the given file extension.
    HasExtension(String),
    /// Lives under the given path prefix.
    InLocation(GlobTerm),
    /// The caller holds the given effective access.
    HasPermission(EffectiveAccess),
}

/// A boolean combination of filter conditions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FilterExpression {
    /// Every child must match.
    All(Vec<FilterExpression>),
    /// At least one child must match.
    Any(Vec<FilterExpression>),
    /// The child must not match.
    Not(Box<FilterExpression>),
    /// A single condition.
    Predicate(FilterPredicate),
}

impl FilterExpression {
    /// Returns every permission predicate in the expression.
    ///
    /// Permission predicates cannot be answered by persistence alone, so the
    /// service evaluates them against domain policy after loading candidates.
    #[must_use]
    pub fn requires_policy_evaluation(&self) -> bool {
        match self {
            Self::Predicate(FilterPredicate::HasPermission(_)) => true,
            Self::Predicate(_) => false,
            Self::Not(inner) => inner.requires_policy_evaluation(),
            Self::All(children) | Self::Any(children) => children
                .iter()
                .any(FilterExpression::requires_policy_evaluation),
        }
    }

    /// Evaluates the complete boolean expression from database predicate
    /// results and the entry's effective access.
    ///
    /// Persistence supplies one boolean for every non-permission predicate in
    /// depth-first source order. Permission predicates are evaluated here,
    /// where domain authorization is authoritative. Keeping both kinds of
    /// truth value in the same expression evaluator preserves exact `or` and
    /// `not` semantics instead of independently weakening each half.
    #[must_use]
    pub fn matches(
        &self,
        effective_access: &[EffectiveAccess],
        database_matches: &[bool],
    ) -> Option<bool> {
        let mut database_matches = database_matches.iter().copied();
        let result = self.matches_inner(effective_access, &mut database_matches)?;
        database_matches.next().is_none().then_some(result)
    }

    fn matches_inner(
        &self,
        effective_access: &[EffectiveAccess],
        database_matches: &mut impl Iterator<Item = bool>,
    ) -> Option<bool> {
        match self {
            Self::Predicate(FilterPredicate::HasPermission(required)) => {
                Some(effective_access.contains(required))
            }
            Self::Predicate(_) => database_matches.next(),
            Self::Not(inner) => inner
                .matches_inner(effective_access, database_matches)
                .map(|matches| !matches),
            Self::All(children) => {
                let mut matches = true;
                for child in children {
                    // Do not short circuit: every database result belongs to a
                    // stable predicate position and must be consumed.
                    matches &= child.matches_inner(effective_access, database_matches)?;
                }
                Some(matches)
            }
            Self::Any(children) => {
                let mut matches = false;
                for child in children {
                    // See `All`: consuming the full sequence also detects a
                    // corrupt or mismatched adapter projection below.
                    matches |= child.matches_inner(effective_access, database_matches)?;
                }
                Some(matches)
            }
        }
    }
}

/// A complete parsed filter: what to match, how to order, how many to take.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FilterQuery {
    /// Conditions, or `None` when the filter only reorders.
    pub expression: Option<FilterExpression>,
    /// Presentation order.
    pub sort: SortOrder,
    /// Chronological result cap.
    pub take: Option<Take>,
}

impl FilterQuery {
    /// Parses one filter expression.
    ///
    /// # Errors
    ///
    /// Returns [`FilterError`] for an oversized, malformed, contradictory, or
    /// unknown filter.
    pub fn parse(input: &str) -> Result<Self, FilterError> {
        if input.len() > MAX_FILTER_LENGTH {
            return Err(FilterError::TooLong);
        }
        let tokens = tokenize(input)?;
        let mut parser = Parser {
            tokens: &tokens,
            position: 0,
            predicates: 0,
            sort: None,
            take: None,
        };
        let expression = parser.parse_query()?;
        Ok(Self {
            expression,
            sort: parser.sort.unwrap_or_default(),
            take: parser.take,
        })
    }

    /// Returns the chronological direction persistence must scan in.
    ///
    /// A `last:`/`first:` modifier decides which end of the timeline the
    /// results come from; `sort:` only decides how they are presented.
    #[must_use]
    pub fn scan_order(&self) -> SortOrder {
        match self.take {
            Some(Take {
                end: TakeEnd::Newest,
                ..
            }) => SortOrder::Newest,
            Some(Take {
                end: TakeEnd::Oldest,
                ..
            }) => SortOrder::Oldest,
            None => self.sort,
        }
    }

    /// Returns whether the scan order differs from the presentation order.
    #[must_use]
    pub fn requires_reversal(&self) -> bool {
        self.scan_order() != self.sort
    }
}

/// Why a filter is invalid.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum FilterError {
    /// The filter exceeds [`MAX_FILTER_LENGTH`].
    #[error("filter is longer than {MAX_FILTER_LENGTH} bytes")]
    TooLong,
    /// The filter contains more than [`MAX_FILTER_PREDICATES`] conditions.
    #[error("filter contains more than {MAX_FILTER_PREDICATES} conditions")]
    TooManyPredicates,
    /// A quoted value is not closed.
    #[error("filter has an unterminated quoted value")]
    UnterminatedQuote,
    /// A parenthesis is not closed, or closes nothing.
    #[error("filter has unbalanced parentheses")]
    UnbalancedParentheses,
    /// An operator or term is missing an operand.
    #[error("filter has a dangling operator")]
    DanglingOperator,
    /// The filter names a key Briefcase does not define.
    #[error("filter key '{key}' is not supported")]
    UnknownKey {
        /// The rejected key.
        key: String,
    },
    /// The value does not fit its key.
    #[error("filter value for '{key}' is invalid")]
    InvalidValue {
        /// The key whose value was rejected.
        key: String,
    },
    /// A result modifier appeared inside a boolean group.
    #[error("filter modifier '{key}' must appear at the top level")]
    MisplacedModifier {
        /// The misplaced modifier key.
        key: String,
    },
    /// The same modifier was given twice with different values.
    #[error("filter modifier '{key}' is repeated")]
    RepeatedModifier {
        /// The repeated modifier key.
        key: String,
    },
    /// The filter selects nothing, such as an empty expression.
    #[error("filter is empty")]
    Empty,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Token {
    Open,
    Close,
    Or,
    And,
    Not,
    Term { key: Option<String>, value: String },
}

fn tokenize(input: &str) -> Result<Vec<Token>, FilterError> {
    let mut tokens = Vec::new();
    let mut characters = input.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            whitespace if whitespace.is_whitespace() => {}
            '(' => tokens.push(Token::Open),
            ')' => tokens.push(Token::Close),
            '-' => tokens.push(Token::Not),
            _ => {
                let mut raw = String::new();
                raw.push(character);
                let mut quote: Option<char> = None;
                while let Some(&next) = characters.peek() {
                    match (quote, next) {
                        (None, '\'' | '"') => {
                            quote = Some(next);
                            raw.push(next);
                            characters.next();
                        }
                        (Some(open), value) if value == open => {
                            quote = None;
                            raw.push(value);
                            characters.next();
                        }
                        (None, ')' | '(') => break,
                        (None, value) if value.is_whitespace() => break,
                        (_, value) => {
                            raw.push(value);
                            characters.next();
                        }
                    }
                }
                if quote.is_some() {
                    return Err(FilterError::UnterminatedQuote);
                }
                tokens.push(word_token(&raw));
            }
        }
    }
    Ok(tokens)
}

fn word_token(raw: &str) -> Token {
    match raw.to_ascii_lowercase().as_str() {
        "or" => return Token::Or,
        "and" => return Token::And,
        "not" => return Token::Not,
        _ => {}
    }
    match raw.split_once(':') {
        Some((key, value)) if !key.is_empty() => Token::Term {
            key: Some(key.to_ascii_lowercase()),
            value: unquote(value),
        },
        _ => Token::Term {
            key: None,
            value: unquote(raw),
        },
    }
}

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    for quote in ['\'', '"'] {
        if trimmed.len() >= 2 && trimmed.starts_with(quote) && trimmed.ends_with(quote) {
            return trimmed[1..trimmed.len() - 1].to_owned();
        }
    }
    trimmed.to_owned()
}

struct Parser<'tokens> {
    tokens: &'tokens [Token],
    position: usize,
    predicates: usize,
    sort: Option<SortOrder>,
    take: Option<Take>,
}

impl Parser<'_> {
    fn parse_query(&mut self) -> Result<Option<FilterExpression>, FilterError> {
        if self.tokens.is_empty() {
            return Err(FilterError::Empty);
        }
        let expression = self.parse_any(true)?;
        if self.position != self.tokens.len() {
            return Err(FilterError::UnbalancedParentheses);
        }
        Ok(expression)
    }

    fn parse_any(&mut self, top_level: bool) -> Result<Option<FilterExpression>, FilterError> {
        let mut alternatives = Vec::new();
        loop {
            let branch = self.parse_all(top_level)?;
            if let Some(branch) = branch {
                alternatives.push(branch);
            }
            if matches!(self.peek(), Some(Token::Or)) {
                self.position += 1;
                if self.position == self.tokens.len() {
                    return Err(FilterError::DanglingOperator);
                }
            } else {
                break;
            }
        }
        Ok(match alternatives.len() {
            0 => None,
            1 => alternatives.pop(),
            _ => Some(FilterExpression::Any(alternatives)),
        })
    }

    fn parse_all(&mut self, top_level: bool) -> Result<Option<FilterExpression>, FilterError> {
        let mut conditions = Vec::new();
        loop {
            match self.peek() {
                None | Some(Token::Or | Token::Close) => break,
                Some(Token::And) => {
                    self.position += 1;
                    if self.position == self.tokens.len() {
                        return Err(FilterError::DanglingOperator);
                    }
                }
                Some(_) => {
                    if let Some(condition) = self.parse_unary(top_level)? {
                        conditions.push(condition);
                    }
                }
            }
        }
        Ok(match conditions.len() {
            0 => None,
            1 => conditions.pop(),
            _ => Some(FilterExpression::All(conditions)),
        })
    }

    fn parse_unary(&mut self, top_level: bool) -> Result<Option<FilterExpression>, FilterError> {
        if matches!(self.peek(), Some(Token::Not)) {
            self.position += 1;
            let inner = self
                .parse_unary(false)?
                .ok_or(FilterError::DanglingOperator)?;
            return Ok(Some(FilterExpression::Not(Box::new(inner))));
        }
        match self.peek() {
            Some(Token::Open) => {
                self.position += 1;
                let inner = self.parse_any(false)?;
                if !matches!(self.peek(), Some(Token::Close)) {
                    return Err(FilterError::UnbalancedParentheses);
                }
                self.position += 1;
                Ok(inner)
            }
            Some(Token::Close) => Err(FilterError::UnbalancedParentheses),
            Some(Token::Term { .. }) => {
                let Some(Token::Term { key, value }) = self.tokens.get(self.position) else {
                    return Err(FilterError::DanglingOperator);
                };
                self.position += 1;
                self.term(key.as_deref(), value, top_level)
            }
            Some(Token::And | Token::Or | Token::Not) | None => Err(FilterError::DanglingOperator),
        }
    }

    fn term(
        &mut self,
        key: Option<&str>,
        value: &str,
        top_level: bool,
    ) -> Result<Option<FilterExpression>, FilterError> {
        let Some(key) = key else {
            // A bare word is the common case: match it against names and
            // extracted content.
            return self
                .predicate(FilterPredicate::Contains(glob(key_of("contains"), value)?))
                .map(Some);
        };
        match key {
            "last" | "first" | "sort" => self.modifier(key, value, top_level).map(|()| None),
            "after" => self
                .predicate(FilterPredicate::ChangedAfter(date(key, value)?))
                .map(Some),
            "before" => self
                .predicate(FilterPredicate::ChangedBefore(date(key, value)?))
                .map(Some),
            "between" => {
                let (start, end) =
                    value
                        .split_once('=')
                        .ok_or_else(|| FilterError::InvalidValue {
                            key: key.to_owned(),
                        })?;
                let (start, end) = (date(key, start)?, date(key, end)?);
                if start > end {
                    return Err(FilterError::InvalidValue {
                        key: key.to_owned(),
                    });
                }
                self.predicate(FilterPredicate::ChangedBetween(start, end))
                    .map(Some)
            }
            "from" => self
                .predicate(FilterPredicate::CreatedBy(actor(key, value)?))
                .map(Some),
            "to" => self
                .predicate(FilterPredicate::SharedWith(actor(key, value)?))
                .map(Some),
            "for" => self
                .predicate(FilterPredicate::AccessibleTo(actor(key, value)?))
                .map(Some),
            "contains" => self
                .predicate(FilterPredicate::Contains(glob(key, value)?))
                .map(Some),
            "has" => self
                .predicate(FilterPredicate::HasContent(glob(key, value)?))
                .map(Some),
            "name" => self
                .predicate(FilterPredicate::NameMatches(glob(key, value)?))
                .map(Some),
            "location" => self
                .predicate(FilterPredicate::InLocation(glob(key, value)?))
                .map(Some),
            "permissions" | "permission" => self
                .predicate(FilterPredicate::HasPermission(permission(key, value)?))
                .map(Some),
            "is" => self.predicate(is_predicate(key, value)?).map(Some),
            _ => Err(FilterError::UnknownKey {
                key: key.to_owned(),
            }),
        }
    }

    /// Records a result modifier, which reorders or caps the whole listing.
    fn modifier(&mut self, key: &str, value: &str, top_level: bool) -> Result<(), FilterError> {
        if !top_level {
            return Err(FilterError::MisplacedModifier {
                key: key.to_owned(),
            });
        }
        if key == "sort" {
            let sort = match value.to_ascii_lowercase().as_str() {
                "newest" => SortOrder::Newest,
                "oldest" => SortOrder::Oldest,
                _ => {
                    return Err(FilterError::InvalidValue {
                        key: key.to_owned(),
                    });
                }
            };
            if self.sort.is_some_and(|existing| existing != sort) {
                return Err(FilterError::RepeatedModifier {
                    key: key.to_owned(),
                });
            }
            self.sort = Some(sort);
            return Ok(());
        }

        let count = value
            .parse::<u16>()
            .ok()
            .filter(|count| (1..=MAX_FILTER_TAKE).contains(count))
            .ok_or_else(|| FilterError::InvalidValue {
                key: key.to_owned(),
            })?;
        let take = Take {
            end: if key == "last" {
                TakeEnd::Newest
            } else {
                TakeEnd::Oldest
            },
            count,
        };
        if self.take.is_some_and(|existing| existing != take) {
            return Err(FilterError::RepeatedModifier {
                key: key.to_owned(),
            });
        }
        self.take = Some(take);
        Ok(())
    }

    fn predicate(&mut self, predicate: FilterPredicate) -> Result<FilterExpression, FilterError> {
        self.predicates = self.predicates.saturating_add(1);
        if self.predicates > MAX_FILTER_PREDICATES {
            return Err(FilterError::TooManyPredicates);
        }
        Ok(FilterExpression::Predicate(predicate))
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }
}

const fn key_of(key: &'static str) -> &'static str {
    key
}

fn glob(key: &str, value: &str) -> Result<GlobTerm, FilterError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().all(|character| character == '*') {
        return Err(FilterError::InvalidValue {
            key: key.to_owned(),
        });
    }
    Ok(GlobTerm(trimmed.to_owned()))
}

fn date(key: &str, value: &str) -> Result<Date, FilterError> {
    Date::parse(value.trim(), DATE_FORMAT).map_err(|_| FilterError::InvalidValue {
        key: key.to_owned(),
    })
}

fn actor(key: &str, value: &str) -> Result<ActorSelector, FilterError> {
    let trimmed = value.trim();
    let inner = trimmed
        .strip_prefix('@')
        .map(str::trim)
        .and_then(|value| value.strip_prefix('{'))
        .and_then(|value| value.strip_suffix('}'))
        .unwrap_or(trimmed);
    let inner = inner.trim();
    if inner.is_empty() {
        return Err(FilterError::InvalidValue {
            key: key.to_owned(),
        });
    }
    // The first segment may name a principal kind. IAM identifiers themselves
    // may contain colons, so only that first segment is ever consumed.
    let (kind, id) = match inner.split_once(':') {
        Some(("carbon", id)) => (Some(ActorKind::Carbon), id),
        Some(("silicon", id)) => (Some(ActorKind::Silicon), id),
        _ => (None, inner),
    };
    let id = id.trim();
    if id.is_empty() || id.len() > 255 {
        return Err(FilterError::InvalidValue {
            key: key.to_owned(),
        });
    }
    Ok(ActorSelector {
        kind,
        id: id.to_owned(),
    })
}

fn permission(key: &str, value: &str) -> Result<EffectiveAccess, FilterError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "read" => Ok(EffectiveAccess::Read),
        "write" => Ok(EffectiveAccess::Write),
        "update" => Ok(EffectiveAccess::Update),
        "delete" => Ok(EffectiveAccess::Delete),
        "manage_permissions" | "manage" => Ok(EffectiveAccess::ManagePermissions),
        _ => Err(FilterError::InvalidValue {
            key: key.to_owned(),
        }),
    }
}

fn is_predicate(key: &str, value: &str) -> Result<FilterPredicate, FilterError> {
    let value = value.trim().trim_start_matches('.').to_ascii_lowercase();
    match value.as_str() {
        "file" => return Ok(FilterPredicate::IsKind(EntryKind::File)),
        "folder" | "directory" => return Ok(FilterPredicate::IsKind(EntryKind::Folder)),
        "image" => return Ok(FilterPredicate::IsRender(RenderKind::Image)),
        "video" => return Ok(FilterPredicate::IsRender(RenderKind::Video)),
        "document" => return Ok(FilterPredicate::IsRender(RenderKind::Document)),
        "spreadsheet" => return Ok(FilterPredicate::IsRender(RenderKind::Spreadsheet)),
        "presentation" => return Ok(FilterPredicate::IsRender(RenderKind::Presentation)),
        "audio" => return Ok(FilterPredicate::IsRender(RenderKind::Audio)),
        "archive" => return Ok(FilterPredicate::IsRender(RenderKind::Archive)),
        "code" => return Ok(FilterPredicate::IsRender(RenderKind::Code)),
        "unsupported" => return Ok(FilterPredicate::IsRender(RenderKind::Unsupported)),
        _ => {}
    }
    let extension_is_plausible = !value.is_empty()
        && value.len() <= 16
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric());
    if extension_is_plausible {
        Ok(FilterPredicate::HasExtension(value))
    } else {
        Err(FilterError::InvalidValue {
            key: key.to_owned(),
        })
    }
}

impl fmt::Display for GlobTerm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use time::macros::date;

    use super::{
        ActorSelector, FilterError, FilterExpression, FilterPredicate, FilterQuery, SortOrder,
        Take, TakeEnd,
    };
    use crate::domain::{actor::ActorKind, entry::EntryKind, permission::EffectiveAccess};

    /// Parses a filter and returns its conditions, or an error when it has none.
    fn conditions(input: &str) -> Result<FilterExpression, FilterError> {
        FilterQuery::parse(input)?
            .expression
            .ok_or(FilterError::Empty)
    }

    fn predicates(expression: &FilterExpression) -> Vec<&FilterPredicate> {
        match expression {
            FilterExpression::Predicate(predicate) => vec![predicate],
            FilterExpression::Not(inner) => predicates(inner),
            FilterExpression::All(children) | FilterExpression::Any(children) => {
                children.iter().flat_map(predicates).collect()
            }
        }
    }

    #[test]
    fn the_contract_example_parses_into_one_expression() -> Result<(), FilterError> {
        let query = FilterQuery::parse(
            "last:5 (between:12-06-2026=12-07-2026 or after:20-08-2026) \
             location:'/private/' (contains:'apple' or contains:'cat') is:md",
        )?;

        assert_eq!(
            query.take,
            Some(Take {
                end: TakeEnd::Newest,
                count: 5
            })
        );
        assert_eq!(query.sort, SortOrder::Newest);
        let expression = query.expression.ok_or(FilterError::Empty)?;
        // between, after, location, two content terms, and the extension.
        assert_eq!(predicates(&expression).len(), 6);
        assert!(matches!(expression, FilterExpression::All(_)));
        Ok(())
    }

    #[test]
    fn terms_default_to_conjunction_and_or_splits_alternatives() -> Result<(), FilterError> {
        let query = FilterQuery::parse("is:file or is:folder")?;
        let expression = query.expression.ok_or(FilterError::Empty)?;
        assert_eq!(
            expression,
            FilterExpression::Any(vec![
                FilterExpression::Predicate(FilterPredicate::IsKind(EntryKind::File)),
                FilterExpression::Predicate(FilterPredicate::IsKind(EntryKind::Folder)),
            ])
        );
        Ok(())
    }

    #[test]
    fn negation_accepts_both_spellings() -> Result<(), FilterError> {
        let dash = FilterQuery::parse("-is:folder")?.expression;
        let word = FilterQuery::parse("not is:folder")?.expression;
        assert_eq!(dash, word);
        assert!(matches!(dash, Some(FilterExpression::Not(_))));
        Ok(())
    }

    #[test]
    fn actor_selectors_keep_colons_inside_identifiers() -> Result<(), FilterError> {
        let query =
            FilterQuery::parse("from:@{carbon:cos:tos} to:@{silicon:agent} for:@{cos:tos}")?;
        let expression = query.expression.ok_or(FilterError::Empty)?;
        let parsed = predicates(&expression);
        assert_eq!(
            parsed,
            vec![
                &FilterPredicate::CreatedBy(ActorSelector {
                    kind: Some(ActorKind::Carbon),
                    id: "cos:tos".to_owned(),
                }),
                &FilterPredicate::SharedWith(ActorSelector {
                    kind: Some(ActorKind::Silicon),
                    id: "agent".to_owned(),
                }),
                &FilterPredicate::AccessibleTo(ActorSelector {
                    kind: None,
                    id: "cos:tos".to_owned(),
                }),
            ]
        );
        Ok(())
    }

    #[test]
    fn a_bare_word_matches_names_and_content() -> Result<(), FilterError> {
        let query = FilterQuery::parse("quarterly")?;
        let expression = query.expression.ok_or(FilterError::Empty)?;
        assert!(matches!(
            expression,
            FilterExpression::Predicate(FilterPredicate::Contains(_))
        ));
        Ok(())
    }

    #[test]
    fn modifiers_decide_scan_direction_independently_of_presentation() -> Result<(), FilterError> {
        let query = FilterQuery::parse("first:3 sort:newest")?;
        assert_eq!(query.scan_order(), SortOrder::Oldest);
        assert_eq!(query.sort, SortOrder::Newest);
        assert!(query.requires_reversal());

        let plain = FilterQuery::parse("sort:oldest")?;
        assert_eq!(plain.scan_order(), SortOrder::Oldest);
        assert!(!plain.requires_reversal());
        Ok(())
    }

    #[test]
    fn glob_patterns_escape_sql_wildcards() -> Result<(), FilterError> {
        let query = FilterQuery::parse("contains:'confirm*' location:'private/100%_final'")?;
        let expression = query.expression.ok_or(FilterError::Empty)?;
        let parsed = predicates(&expression);
        let FilterPredicate::Contains(term) = parsed[0] else {
            panic!("first predicate should be a content match");
        };
        assert_eq!(term.like_pattern(), "%confirm%");
        let FilterPredicate::InLocation(location) = parsed[1] else {
            panic!("second predicate should be a location match");
        };
        assert_eq!(location.prefix_pattern(), r"private/100\%\_final%");
        Ok(())
    }

    #[test]
    fn dates_use_the_contracted_day_first_format() -> Result<(), FilterError> {
        let query = FilterQuery::parse("between:12-06-2026=12-07-2026")?;
        let expression = query.expression.ok_or(FilterError::Empty)?;
        assert_eq!(
            predicates(&expression),
            vec![&FilterPredicate::ChangedBetween(
                date!(2026 - 06 - 12),
                date!(2026 - 07 - 12)
            )]
        );
        assert_eq!(
            FilterQuery::parse("after:2026-06-12"),
            Err(FilterError::InvalidValue {
                key: "after".to_owned()
            })
        );
        Ok(())
    }

    #[test]
    fn permission_predicates_are_evaluated_by_policy_not_persistence() -> Result<(), FilterError> {
        let query = FilterQuery::parse("permissions:delete")?;
        let expression = query.expression.ok_or(FilterError::Empty)?;
        assert!(expression.requires_policy_evaluation());
        assert_eq!(
            expression.matches(&[EffectiveAccess::Read, EffectiveAccess::Delete], &[]),
            Some(true)
        );
        assert_eq!(
            expression.matches(&[EffectiveAccess::Read], &[]),
            Some(false)
        );

        let negated = conditions("-permissions:delete")?;
        assert_eq!(
            negated.matches(&[EffectiveAccess::Delete], &[]),
            Some(false)
        );
        assert_eq!(negated.matches(&[EffectiveAccess::Read], &[]), Some(true));
        Ok(())
    }

    #[test]
    fn mixed_database_and_permission_booleans_are_evaluated_exactly() -> Result<(), FilterError> {
        let expression = conditions("name:'apple*' or permissions:delete")?;
        assert_eq!(
            expression.matches(&[EffectiveAccess::Read], &[true]),
            Some(true),
            "the database side of OR may satisfy the expression"
        );
        assert_eq!(
            expression.matches(&[EffectiveAccess::Read, EffectiveAccess::Delete], &[false]),
            Some(true),
            "the permission side of OR may satisfy the expression"
        );
        assert_eq!(
            expression.matches(&[EffectiveAccess::Read], &[false]),
            Some(false)
        );

        let negated = conditions("not (name:'apple*' or permissions:delete)")?;
        assert_eq!(
            negated.matches(&[EffectiveAccess::Read], &[false]),
            Some(true)
        );
        assert_eq!(
            negated.matches(&[EffectiveAccess::Read], &[true]),
            Some(false)
        );
        assert_eq!(
            negated.matches(&[EffectiveAccess::Delete], &[false]),
            Some(false)
        );

        assert_eq!(expression.matches(&[EffectiveAccess::Read], &[]), None);
        assert_eq!(
            expression.matches(&[EffectiveAccess::Read], &[false, true]),
            None
        );
        Ok(())
    }

    #[test]
    fn structural_and_semantic_mistakes_are_rejected() {
        assert_eq!(FilterQuery::parse(""), Err(FilterError::Empty));
        assert_eq!(
            FilterQuery::parse("(is:file"),
            Err(FilterError::UnbalancedParentheses)
        );
        assert_eq!(
            FilterQuery::parse("is:file)"),
            Err(FilterError::UnbalancedParentheses)
        );
        assert_eq!(
            FilterQuery::parse("is:file or"),
            Err(FilterError::DanglingOperator)
        );
        assert_eq!(
            FilterQuery::parse("contains:'unclosed"),
            Err(FilterError::UnterminatedQuote)
        );
        assert_eq!(
            FilterQuery::parse("colour:red"),
            Err(FilterError::UnknownKey {
                key: "colour".to_owned()
            })
        );
        assert_eq!(
            FilterQuery::parse("(last:5)"),
            Err(FilterError::MisplacedModifier {
                key: "last".to_owned()
            })
        );
        assert_eq!(
            FilterQuery::parse("last:5 last:6"),
            Err(FilterError::RepeatedModifier {
                key: "last".to_owned()
            })
        );
        assert_eq!(
            FilterQuery::parse("between:12-07-2026=12-06-2026"),
            Err(FilterError::InvalidValue {
                key: "between".to_owned()
            })
        );
        assert_eq!(
            FilterQuery::parse("last:0"),
            Err(FilterError::InvalidValue {
                key: "last".to_owned()
            })
        );
    }
}
