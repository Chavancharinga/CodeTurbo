use coturbo_agent::approval::{ApprovalConfig, ApprovalMode, Policy, SessionGrants, Verdict, glob_match, resolve};
use coturbo_tools::Tier;
use serde_json::json;
use std::collections::BTreeMap;

fn cfg(mode: ApprovalMode, rules: &[(&str, &str)]) -> ApprovalConfig {
    let map: BTreeMap<String, String> = rules.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    ApprovalConfig::new(mode, &map).unwrap()
}

fn r(c: &ApprovalConfig, g: &SessionGrants, tool: &str, input: serde_json::Value) -> Verdict {
    resolve(tool, coturbo_tools::tier(tool), &input, c, g, false)
}

#[test]
fn modes_cap_the_auto_approved_tier() {
    let g = SessionGrants::default();
    let cases = [
        (ApprovalMode::AlwaysAsk, "read_file", Verdict::Allow),
        (ApprovalMode::AlwaysAsk, "edit_file", Verdict::Prompt),
        (ApprovalMode::AlwaysAsk, "bash", Verdict::Prompt),
        (ApprovalMode::Write, "read_file", Verdict::Allow),
        (ApprovalMode::Write, "edit_file", Verdict::Allow),
        (ApprovalMode::Write, "bash", Verdict::Prompt),
        (ApprovalMode::Yolo, "bash", Verdict::Allow),
        (ApprovalMode::Yolo, "mcp__x", Verdict::Allow),
    ];
    for (mode, tool, want) in cases {
        assert_eq!(r(&cfg(mode, &[]), &g, tool, json!({"command": "ls"})), want, "{mode:?} {tool}");
    }
}

#[test]
fn user_deny_beats_yolo_and_allow_beats_always_ask() {
    let g = SessionGrants::default();
    let c = cfg(ApprovalMode::Yolo, &[("bash", "deny")]);
    assert!(matches!(r(&c, &g, "bash", json!({"command": "ls"})), Verdict::Deny(_)));
    let c = cfg(ApprovalMode::AlwaysAsk, &[("edit_file", "allow")]);
    assert_eq!(r(&c, &g, "edit_file", json!({})), Verdict::Allow);
    let c = cfg(ApprovalMode::Yolo, &[("write_file", "prompt")]);
    assert_eq!(r(&c, &g, "write_file", json!({})), Verdict::Prompt);
}

#[test]
fn bash_patterns_longest_wins_ties_go_to_the_strictest() {
    let g = SessionGrants::default();
    let c = cfg(
        ApprovalMode::Write,
        &[("bash:cargo *", "allow"), ("bash:cargo publish*", "deny"), ("bash", "prompt")],
    );
    assert_eq!(r(&c, &g, "bash", json!({"command": "cargo test -q"})), Verdict::Allow);
    assert!(matches!(r(&c, &g, "bash", json!({"command": "cargo publish"})), Verdict::Deny(_)));
    assert_eq!(r(&c, &g, "bash", json!({"command": "rm -rf x"})), Verdict::Prompt);

    let tie = cfg(ApprovalMode::Yolo, &[("bash:git *", "allow"), ("bash:* push", "deny")]);
    // "git *" and "* push" have the same length: deny wins.
    assert!(matches!(r(&tie, &g, "bash", json!({"command": "git push"})), Verdict::Deny(_)));
}

#[test]
fn session_grants_skip_the_prompt() {
    let c = cfg(ApprovalMode::AlwaysAsk, &[]);
    let mut g = SessionGrants::default();
    g.grant("edit_file", &json!({}));
    g.grant("bash", &json!({"command": "cargo build"}));
    assert_eq!(r(&c, &g, "edit_file", json!({})), Verdict::Allow);
    assert_eq!(r(&c, &g, "bash", json!({"command": "cargo test"})), Verdict::Allow);
    assert_eq!(r(&c, &g, "bash", json!({"command": "rm x"})), Verdict::Prompt);
    assert_eq!(r(&c, &g, "write_file", json!({})), Verdict::Prompt);
    // A user deny still wins over a session grant.
    let c = cfg(ApprovalMode::AlwaysAsk, &[("edit_file", "deny")]);
    assert!(matches!(r(&c, &g, "edit_file", json!({})), Verdict::Deny(_)));
}

#[test]
fn plan_mode_is_read_only_except_plan_files() {
    let c = cfg(ApprovalMode::Yolo, &[("edit_file", "allow")]);
    let g = SessionGrants::default();
    let v = resolve("edit_file", Tier::Write, &json!({"path": "src/lib.rs"}), &c, &g, true);
    assert!(matches!(v, Verdict::Deny(m) if m.contains("plan mode")));
    let v = resolve("bash", Tier::Exec, &json!({"command": "ls"}), &c, &g, true);
    assert!(matches!(v, Verdict::Deny(_)));
    let v = resolve("write_file", Tier::Write, &json!({"path": ".coturbo/plans/x-plan.md"}), &c, &g, true);
    assert_eq!(v, Verdict::Allow);
    let v = resolve("write_file", Tier::Write, &json!({"path": "./.coturbo\\plans/x-plan.md"}), &c, &g, true);
    assert_eq!(v, Verdict::Allow);
    let v = resolve("write_file", Tier::Write, &json!({"path": ".coturbo/plans/../../src/x-plan.md"}), &c, &g, true);
    assert!(matches!(v, Verdict::Deny(_)));
    let v = resolve("read_file", Tier::Read, &json!({"path": "src/lib.rs"}), &c, &g, true);
    assert_eq!(v, Verdict::Allow);
}

#[test]
fn plan_slugs() {
    use coturbo_agent::approval::plan_slug;
    assert_eq!(plan_slug(".coturbo/plans/auth-fix-plan.md").as_deref(), Some("auth-fix"));
    assert_eq!(plan_slug(".coturbo/plans/-plan.md"), None);
    assert_eq!(plan_slug(".coturbo/plans/a b-plan.md"), None);
    assert_eq!(plan_slug(".coturbo/plans/sub/x-plan.md"), None);
    assert_eq!(plan_slug("src/x-plan.md"), None);
}

#[test]
fn config_parsing_is_strict() {
    let bad: BTreeMap<String, String> = [("bash".to_string(), "maybe".to_string())].into();
    assert!(ApprovalConfig::new(ApprovalMode::Write, &bad).is_err());
    let empty: BTreeMap<String, String> = [("bash:".to_string(), "allow".to_string())].into();
    assert!(ApprovalConfig::new(ApprovalMode::Write, &empty).is_err());
    assert_eq!("always-ask".parse::<ApprovalMode>(), Ok(ApprovalMode::AlwaysAsk));
    assert_eq!("yolo".parse::<ApprovalMode>(), Ok(ApprovalMode::Yolo));
    assert!("never".parse::<ApprovalMode>().is_err());
    assert_eq!("ALLOW".parse::<Policy>(), Ok(Policy::Allow));
}

#[test]
fn glob_matches_whole_string() {
    assert!(glob_match("cargo test*", "cargo test -q"));
    assert!(glob_match("cargo test*", "cargo test"));
    assert!(!glob_match("cargo test*", "xcargo test"));
    assert!(glob_match("*push*", "git push origin"));
    assert!(glob_match("ls", "ls"));
    assert!(!glob_match("ls", "ls -la"));
    assert!(glob_match("a*b*c", "aXXbYYc"));
    assert!(!glob_match("a*b*c", "aXXbYY"));
}
