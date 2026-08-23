//! Message filtering: compiled allowlist/denylist dimensions.
//!
//! Pure evaluation logic — no I/O, no state. `CompiledFilters::compile`
//! turns config into matchers; `permits` is the single decision point
//! the engine calls for every message/event.

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use quiver_twitch::Badge;
use regex::Regex;

use crate::config::{FiltersConfig, ListFilter, Mode};

/// Compiled filters shared between the server (initial/reload) and the
/// engine pump. `None` = no filtering configured.
pub(crate) type SharedCompiled = Arc<RwLock<Option<CompiledFilters>>>;

/// Message kinds as they appear in filter config AND on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgKind {
    Message,
    Command,
    Sub,
    GiftSub,
    MysteryGift,
    Raid,
}

impl MsgKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "message" => Some(Self::Message),
            "command" => Some(Self::Command),
            "sub" => Some(Self::Sub),
            "gift_sub" => Some(Self::GiftSub),
            "mystery_gift" => Some(Self::MysteryGift),
            "raid" => Some(Self::Raid),
            _ => None,
        }
    }

    /// Canonical wire/config name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Command => "command",
            Self::Sub => "sub",
            Self::GiftSub => "gift_sub",
            Self::MysteryGift => "mystery_gift",
            Self::Raid => "raid",
        }
    }

    /// Classify a chat text against the configured prefixes.
    fn classify(prefixes: &[String], text: &str) -> Self {
        let is_command = !text.is_empty()
            && prefixes
                .iter()
                .any(|p| !p.is_empty() && text.starts_with(p.as_str()));
        if is_command {
            Self::Command
        } else {
            Self::Message
        }
    }
}

#[derive(Debug)]
struct RegexDim {
    allowlist: bool,
    res: Vec<Regex>,
}

impl RegexDim {
    fn compile(dim: &ListFilter) -> Result<Self, String> {
        let mut res = Vec::new();
        for item in &dim.items {
            res.push(Regex::new(item).map_err(|e| format!("{item:?}: {e}"))?);
        }
        Ok(Self {
            allowlist: dim.mode == Mode::Allowlist,
            res,
        })
    }

    /// None = inactive (empty items).
    fn passes(&self, value: &str) -> bool {
        if self.res.is_empty() {
            return true;
        }
        let matched = self.res.iter().any(|r| r.is_match(value));
        if self.allowlist { matched } else { !matched }
    }
}

#[derive(Debug)]
struct ExactDim {
    allowlist: bool,
    set: HashSet<String>,
}

impl ExactDim {
    fn new(dim: &ListFilter) -> Self {
        Self {
            allowlist: dim.mode == Mode::Allowlist,
            set: dim.items.iter().cloned().collect(),
        }
    }

    fn passes(&self, value: &str) -> bool {
        if self.set.is_empty() {
            return true;
        }
        let matched = self.set.contains(value);
        if self.allowlist { matched } else { !matched }
    }
}

/// Everything the permit decision needs for one message/event.
/// Badges are the sender's badge ids (empty for events in v1).
pub struct PermitCtx<'a> {
    pub kind: MsgKind,
    pub login: &'a str,
    pub display_name: &'a str,
    pub user_id: &'a str,
    pub badges: &'a [Badge],
    pub content: &'a str,
}

/// Compiled, ready-to-evaluate filters. `None` fields are inactive.
#[derive(Debug, Default)]
pub struct CompiledFilters {
    prefixes: Vec<String>,
    display_name: Option<RegexDim>,
    username: Option<RegexDim>,
    user_id: Option<ExactDim>,
    content: Option<RegexDim>,
    message_type: Option<ExactDim>,
    role: Option<ExactDim>,
}

impl CompiledFilters {
    /// Compile config into matchers. Errors carry the offending pattern.
    pub fn compile(cfg: &FiltersConfig) -> Result<Self, String> {
        let compile_regex_dim = |dim: &Option<ListFilter>| -> Result<Option<RegexDim>, String> {
            dim.as_ref().map(RegexDim::compile).transpose()
        };
        let exact =
            |dim: &Option<ListFilter>| -> Option<ExactDim> { dim.as_ref().map(ExactDim::new) };

        Ok(Self {
            prefixes: cfg.command_prefixes.clone(),
            display_name: compile_regex_dim(&cfg.display_name)?,
            username: compile_regex_dim(&cfg.username)?,
            user_id: exact(&cfg.user_id),
            content: compile_regex_dim(&cfg.content)?,
            message_type: exact(&cfg.message_type),
            role: exact(&cfg.role),
        })
    }

    /// The single decision point. True = render.
    ///
    /// Dimensions combine with AND; inactive dimensions always pass.
    /// An empty items list makes its dimension inactive by construction.
    pub fn permits(&self, ctx: &PermitCtx) -> bool {
        if let Some(dim) = &self.display_name
            && !dim.passes(ctx.display_name)
        {
            return false;
        }
        if let Some(dim) = &self.username
            && !dim.passes(ctx.login)
        {
            return false;
        }
        if let Some(dim) = &self.user_id
            && !dim.passes(ctx.user_id)
        {
            return false;
        }
        if let Some(dim) = &self.content
            && !dim.passes(ctx.content)
        {
            return false;
        }
        if let Some(dim) = &self.message_type
            && !dim.passes(ctx.kind.name())
        {
            return false;
        }
        if let Some(dim) = &self.role {
            // Pass when ANY carried badge matches the dimension's rules.
            let badge_ids: Vec<&str> = ctx.badges.iter().map(|b| b.id.as_str()).collect();
            let passes = if dim.set.is_empty() {
                true
            } else if dim.allowlist {
                badge_ids.iter().any(|id| dim.set.contains(*id))
            } else {
                !badge_ids.iter().any(|id| dim.set.contains(*id))
            };
            if !passes {
                return false;
            }
        }
        true
    }

    /// Classify chat text into a kind using configured prefixes.
    pub fn classify_chat(&self, text: &str) -> MsgKind {
        MsgKind::classify(&self.prefixes, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ListFilter;

    fn list(mode: Mode, items: &[&str]) -> ListFilter {
        ListFilter {
            mode,
            items: items.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn cfg() -> FiltersConfig {
        FiltersConfig::default()
    }

    fn ctx<'a>(
        kind: MsgKind,
        login: &'a str,
        display: &'a str,
        id: &'a str,
        badges: &'a [Badge],
        content: &'a str,
    ) -> PermitCtx<'a> {
        PermitCtx {
            kind,
            login,
            display_name: display,
            user_id: id,
            badges,
            content,
        }
    }

    // ---- per-dimension basics ---------------------------------------------

    #[test]
    fn no_filters_permit_everything() {
        let f = CompiledFilters::compile(&cfg()).unwrap();
        assert!(f.permits(&ctx(MsgKind::Message, "a", "A", "1", &[], "hi")));
    }

    #[test]
    fn denylist_drops_matching_display_name() {
        let mut c = cfg();
        c.display_name = Some(list(Mode::Denylist, &["^Stream.*"]));
        let f = CompiledFilters::compile(&c).unwrap();
        assert!(!f.permits(&ctx(
            MsgKind::Message,
            "streamelements",
            "StreamElements",
            "1",
            &[],
            ""
        )));
        assert!(f.permits(&ctx(
            MsgKind::Message,
            "melodieee__",
            "Melodieee__",
            "2",
            &[],
            ""
        )));
    }

    #[test]
    fn allowlist_requires_a_match() {
        let mut c = cfg();
        c.username = Some(list(Mode::Allowlist, &["^vip_"]));
        let f = CompiledFilters::compile(&c).unwrap();
        assert!(f.permits(&ctx(MsgKind::Message, "vip_friend", "F", "1", &[], "")));
        assert!(!f.permits(&ctx(MsgKind::Message, "random", "R", "2", &[], "")));
    }

    #[test]
    fn empty_items_make_dimension_inactive() {
        // Empty ALLOWLIST would mean "drop everything" — refused: inactive.
        let mut c = cfg();
        c.username = Some(list(Mode::Allowlist, &[]));
        let f = CompiledFilters::compile(&c).unwrap();
        assert!(f.permits(&ctx(MsgKind::Message, "anyone", "Anyone", "1", &[], "")));
    }

    #[test]
    fn user_id_is_exact_not_regex() {
        let mut c = cfg();
        // Regex metachars must be LITERAL here: "7*" matches only the id
        // literally spelled "7*", never "71092938".
        c.user_id = Some(list(Mode::Denylist, &["7*", "123"]));
        let f = CompiledFilters::compile(&c).unwrap();
        assert!(f.permits(&ctx(MsgKind::Message, "u", "U", "71092938", &[], "")));
        assert!(!f.permits(&ctx(MsgKind::Message, "u", "U", "123", &[], "")));
        assert!(!f.permits(&ctx(MsgKind::Message, "u", "U", "7*", &[], "")));
    }

    #[test]
    fn content_denylist_case_insensitive_via_inline_flag() {
        let mut c = cfg();
        c.content = Some(list(Mode::Denylist, &["(?i)free money"]));
        let f = CompiledFilters::compile(&c).unwrap();
        assert!(!f.permits(&ctx(MsgKind::Message, "u", "U", "1", &[], "FREE MONEY now")));
        assert!(f.permits(&ctx(MsgKind::Message, "u", "U", "1", &[], "hello")));
    }

    // ---- kinds + prefixes ---------------------------------------------------

    #[test]
    fn command_classification_honors_configured_prefixes() {
        let mut c = cfg(); // default ["!"]
        let f = CompiledFilters::compile(&c).unwrap();
        assert_eq!(f.classify_chat("!so xqc"), MsgKind::Command);
        assert_eq!(f.classify_chat("hello"), MsgKind::Message);

        c.command_prefixes = vec!["!".into(), "?".into()];
        let f2 = CompiledFilters::compile(&c).unwrap();
        assert_eq!(f2.classify_chat("?so"), MsgKind::Command);
    }

    #[test]
    fn message_type_filter_drops_commands_only() {
        let mut c = cfg();
        c.message_type = Some(list(Mode::Denylist, &["command"]));
        let f = CompiledFilters::compile(&c).unwrap();
        assert!(!f.permits(&ctx(MsgKind::Command, "u", "U", "1", &[], "!so")));
        assert!(f.permits(&ctx(MsgKind::Sub, "u", "U", "1", &[], "")));
        assert!(f.permits(&ctx(MsgKind::Message, "u", "U", "1", &[], "normal")));
    }

    // ---- roles ----------------------------------------------------------------

    #[test]
    fn role_allowlist_keeps_only_listed_badges() {
        let mut c = cfg();
        c.role = Some(list(Mode::Allowlist, &["moderator", "vip"]));
        let f = CompiledFilters::compile(&c).unwrap();

        let mod_badges = [Badge {
            id: "moderator".into(),
            version: "1".into(),
        }];
        let plain: Vec<Badge> = Vec::new();
        assert!(f.permits(&ctx(MsgKind::Message, "m", "M", "1", &mod_badges, "")));
        assert!(!f.permits(&ctx(MsgKind::Message, "p", "P", "2", &plain, "")));
    }

    #[test]
    fn role_denylist_removes_listed_badges_only() {
        let mut c = cfg();
        c.role = Some(list(Mode::Denylist, &["subscriber"]));
        let f = CompiledFilters::compile(&c).unwrap();

        let sub = [Badge {
            id: "subscriber".into(),
            version: "54".into(),
        }];
        let mixed = [
            Badge {
                id: "subscriber".into(),
                version: "54".into(),
            },
            Badge {
                id: "moderator".into(),
                version: "1".into(),
            },
        ];
        assert!(!f.permits(&ctx(MsgKind::Message, "s", "S", "1", &sub, "")));
        // Carrying subscriber among others → still denied (denylist = any hit).
        assert!(!f.permits(&ctx(MsgKind::Message, "m", "M", "2", &mixed, "")));
    }

    // ---- composition ------------------------------------------------------------

    /// Ground truth BY HAND: username denylist hits AND role allowlist
    /// fails → dropped. Fixing either one lets it through (AND semantics).
    #[test]
    fn dimensions_combine_with_and() {
        let mut c = cfg();
        c.username = Some(list(Mode::Denylist, &["^bot"]));
        c.role = Some(list(Mode::Allowlist, &["moderator"]));
        let f = CompiledFilters::compile(&c).unwrap();

        let mod_badges = [Badge {
            id: "moderator".into(),
            version: "1".into(),
        }];
        // bot with moderator badge: username dimension drops it.
        assert!(!f.permits(&ctx(
            MsgKind::Message,
            "botguy",
            "BotGuy",
            "1",
            &mod_badges,
            ""
        )));
        // human without badge: role dimension drops it.
        assert!(!f.permits(&ctx(MsgKind::Message, "human", "Human", "2", &[], "")));
        // human moderator: both pass.
        assert!(f.permits(&ctx(
            MsgKind::Message,
            "realmod",
            "RealMod",
            "3",
            &mod_badges,
            ""
        )));
    }

    #[test]
    fn invalid_regex_is_a_compile_error() {
        let mut c = cfg();
        c.content = Some(list(Mode::Denylist, &["[unclosed"]));
        assert!(CompiledFilters::compile(&c).is_err());
    }
}
