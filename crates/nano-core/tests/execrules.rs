use nano_core::execrules::{
    AmendmentKind, ApprovalRuleChoice, ComplexReason, PatternToken, PrefixRule, RuleDecision,
    RuleMatch, RuleSet, RuleSource, RuleStoreError, RuleVerdict, ShellGrammar, TokenizeOutcome,
    append_amendment, configured_rules_path, load_rules, mint_amendment, mint_approval_rule,
    rules_path, tokenize,
};
use pretty_assertions::assert_eq;
use std::fs;
use std::path::Path;

fn rule(pattern: &[&str], exact: bool, decision: RuleDecision) -> PrefixRule {
    PrefixRule {
        pattern: pattern
            .iter()
            .map(|token| PatternToken::Single((*token).to_owned()))
            .collect(),
        exact,
        decision,
        justification: None,
        added_at: None,
        source: None,
    }
}

fn clean(command: &str, shell: ShellGrammar) -> Vec<Vec<String>> {
    match tokenize(command, shell) {
        TokenizeOutcome::Clean(segments) => segments,
        other => panic!("expected Clean for {command:?}, got {other:?}"),
    }
}

fn complex(command: &str, shell: ShellGrammar) -> ComplexReason {
    match tokenize(command, shell) {
        TokenizeOutcome::Complex { reason, .. } => reason,
        other => panic!("expected Complex for {command:?}, got {other:?}"),
    }
}

#[test]
fn posix_positive_grammar_golden_table() {
    let cases = [
        ("git status", vec![vec!["git", "status"]]),
        ("printf 'hello world'", vec![vec!["printf", "hello world"]]),
        (
            "git status && cargo test || echo failed; pwd | wc -l",
            vec![
                vec!["git", "status"],
                vec!["cargo", "test"],
                vec!["echo", "failed"],
                vec!["pwd"],
                vec!["wc", "-l"],
            ],
        ),
        (
            "tool a,b key:value x=y /tmp/a",
            vec![vec!["tool", "a,b", "key:value", "x=y", "/tmp/a"]],
        ),
    ];
    for (command, expected) in cases {
        assert_eq!(
            clean(command, ShellGrammar::PosixSh),
            expected
                .into_iter()
                .map(|segment| segment.into_iter().map(String::from).collect())
                .collect::<Vec<Vec<String>>>()
        );
    }
}

#[test]
fn posix_every_non_positive_form_is_complex() {
    for command in [
        "echo \"x\"",
        r"echo x\ y",
        "echo $HOME",
        "echo `pwd`",
        "echo $(pwd)",
        "(pwd)",
        "echo x > out",
        "echo *.rs",
        "echo ~",
        "echo x # comment",
        "echo x\ny",
        "echo ?",
        "echo [x]",
    ] {
        assert_eq!(
            complex(command, ShellGrammar::PosixSh),
            ComplexReason::UnsupportedSyntax,
            "{command:?}"
        );
    }
}

#[test]
fn cmd_positive_grammar_golden_table() {
    let cases = [
        (r"git status", vec![vec!["git", "status"]]),
        (
            r"git status && cargo test || echo failed & dir | findstr src",
            vec![
                vec!["git", "status"],
                vec!["cargo", "test"],
                vec!["echo", "failed"],
                vec!["dir"],
                vec!["findstr", "src"],
            ],
        ),
        (
            r"tool C:\work\x /flag:value key=value a/b +x @file",
            vec![vec![
                "tool",
                r"C:\work\x",
                "/flag:value",
                "key=value",
                "a/b",
                "+x",
                "@file",
            ]],
        ),
    ];
    for (command, expected) in cases {
        assert_eq!(
            clean(command, ShellGrammar::CmdExe),
            expected
                .into_iter()
                .map(|segment| segment.into_iter().map(String::from).collect())
                .collect::<Vec<Vec<String>>>()
        );
    }
}

#[test]
fn cmd_every_non_positive_form_is_complex() {
    for command in [
        "echo \"x\"",
        "echo ^x",
        "echo %PATH%",
        "echo !X!",
        "(echo x)",
        "echo x > out",
        "echo x,y",
        "echo x;y",
    ] {
        assert_eq!(
            complex(command, ShellGrammar::CmdExe),
            ComplexReason::UnsupportedSyntax,
            "{command:?}"
        );
    }
    assert_eq!(
        complex("X=value echo x", ShellGrammar::CmdExe),
        ComplexReason::CmdAssignment
    );
    assert_eq!(
        complex("if exist x echo y", ShellGrammar::CmdExe),
        ComplexReason::CmdControlKeyword
    );
    assert_eq!(
        complex("for x in y", ShellGrammar::CmdExe),
        ComplexReason::CmdControlKeyword
    );
}

#[test]
fn parser_caps_fail_closed() {
    assert_eq!(
        complex(&"x".repeat(4097), ShellGrammar::PosixSh),
        ComplexReason::CommandTooLong
    );
    let tokens = std::iter::repeat_n("x", 65).collect::<Vec<_>>().join(" ");
    assert_eq!(
        complex(&tokens, ShellGrammar::PosixSh),
        ComplexReason::TooManyTokens
    );
    let segments = std::iter::repeat_n("x", 33).collect::<Vec<_>>().join(";");
    assert_eq!(
        complex(&segments, ShellGrammar::PosixSh),
        ComplexReason::TooManySegments
    );
}

#[test]
fn exact_anchor_and_alternatives_are_narrow() {
    let rules = RuleSet::new(vec![
        rule(
            &["git", "push", "origin", "main"],
            true,
            RuleDecision::Allow,
        ),
        PrefixRule {
            pattern: vec![
                PatternToken::Single("git".into()),
                PatternToken::Alts(vec!["status".into(), "diff".into()]),
            ],
            exact: false,
            decision: RuleDecision::Allow,
            justification: None,
            added_at: None,
            source: None,
        },
    ])
    .unwrap();
    assert_eq!(
        rules.evaluate(ShellGrammar::PosixSh, "git push origin main"),
        RuleVerdict::Allow
    );
    assert_eq!(
        rules.evaluate(ShellGrammar::PosixSh, "git push origin main --force"),
        RuleVerdict::Prompt
    );
    assert_eq!(
        rules.evaluate(ShellGrammar::PosixSh, "git status -sb"),
        RuleVerdict::Allow
    );
    assert_eq!(
        rules.evaluate(ShellGrammar::PosixSh, "git branch"),
        RuleVerdict::Prompt
    );
}

#[test]
fn invalid_patterns_are_rejected_structurally() {
    for pattern in [
        Vec::new(),
        vec![PatternToken::Alts(vec!["git".into()])],
        vec![PatternToken::Single(String::new())],
        vec![
            PatternToken::Single("git".into()),
            PatternToken::Alts(Vec::new()),
        ],
        vec![
            PatternToken::Single("git".into()),
            PatternToken::Alts(vec![String::new()]),
        ],
    ] {
        let invalid = PrefixRule {
            pattern,
            exact: false,
            decision: RuleDecision::Allow,
            justification: None,
            added_at: None,
            source: None,
        };
        assert!(RuleSet::new(vec![invalid]).is_err());
    }
}

#[test]
fn strictest_wins_and_allow_requires_every_segment() {
    let rules = RuleSet::new(vec![
        rule(&["git", "status"], true, RuleDecision::Allow),
        rule(&["rm"], false, RuleDecision::Deny),
        rule(&["cargo", "test"], true, RuleDecision::Prompt),
    ])
    .unwrap();
    let denied = rules.evaluate_with_matches(ShellGrammar::PosixSh, "git status && rm -rf x");
    assert_eq!(denied.verdict(), RuleVerdict::Deny);
    assert_eq!(denied.matched.len(), 2);
    let deny_match = denied
        .matched
        .iter()
        .find(|matched| matched.decision == RuleDecision::Deny)
        .unwrap();
    assert_eq!(deny_match.rule_index, 1);
    assert_eq!(deny_match.matched_prefix, vec!["rm"]);
    assert_eq!(
        rules.evaluate(ShellGrammar::PosixSh, "git status && echo ok"),
        RuleVerdict::Prompt
    );

    for rules in [
        vec![
            rule(&["git"], false, RuleDecision::Allow),
            rule(&["git"], false, RuleDecision::Deny),
        ],
        vec![
            rule(&["git"], false, RuleDecision::Deny),
            rule(&["git"], false, RuleDecision::Allow),
        ],
    ] {
        assert_eq!(
            RuleSet::new(rules)
                .unwrap()
                .evaluate(ShellGrammar::PosixSh, "git status"),
            RuleVerdict::Deny
        );
    }
    assert_eq!(
        rules.evaluate(ShellGrammar::PosixSh, "git status && cargo test"),
        RuleVerdict::Prompt
    );
}

#[test]
fn complex_syntax_has_prompt_floor_but_deny_still_wins() {
    let rules = RuleSet::new(vec![
        rule(&["echo"], false, RuleDecision::Allow),
        rule(&["rm"], false, RuleDecision::Deny),
    ])
    .unwrap();
    let prompt = rules.evaluate_with_matches(ShellGrammar::PosixSh, "echo $HOME");
    assert_eq!(prompt.verdict(), RuleVerdict::Prompt);
    assert!(!prompt.amendable);
    assert_eq!(
        prompt.complex_reason,
        Some(ComplexReason::UnsupportedSyntax)
    );
    assert_eq!(
        rules.evaluate(ShellGrammar::PosixSh, "echo $HOME && rm -rf x"),
        RuleVerdict::Deny
    );
}

#[test]
fn nested_shell_layers_require_unanimity_and_opaque_forms_prompt() {
    let rules = RuleSet::new(vec![
        rule(&["sh", "-c", "echo"], true, RuleDecision::Allow),
        rule(&["echo"], true, RuleDecision::Allow),
        rule(&["cmd", "/c", "dir"], true, RuleDecision::Allow),
        rule(&["dir"], true, RuleDecision::Allow),
    ])
    .unwrap();
    assert_eq!(
        rules.evaluate(ShellGrammar::PosixSh, "sh -c echo"),
        RuleVerdict::Allow
    );
    let quoted_rules = RuleSet::new(vec![
        rule(&["sh", "-c", "echo hello"], true, RuleDecision::Allow),
        rule(&["echo", "hello"], true, RuleDecision::Allow),
    ])
    .unwrap();
    assert_eq!(
        quoted_rules.evaluate(ShellGrammar::PosixSh, "sh -c 'echo hello'"),
        RuleVerdict::Allow
    );
    let nested_strictest = RuleSet::new(vec![
        rule(&["sh", "-c", "echo ok && rm x"], true, RuleDecision::Allow),
        rule(&["echo", "ok"], true, RuleDecision::Allow),
        rule(&["rm"], false, RuleDecision::Deny),
    ])
    .unwrap();
    assert_eq!(
        nested_strictest.evaluate(ShellGrammar::PosixSh, "sh -c 'echo ok && rm x'"),
        RuleVerdict::Deny
    );
    assert_eq!(
        rules.evaluate(ShellGrammar::CmdExe, "cmd /c dir"),
        RuleVerdict::Allow
    );
    assert_eq!(
        rules.evaluate(ShellGrammar::CmdExe, "sh -c echo"),
        RuleVerdict::Allow
    );

    let inner_only = RuleSet::new(vec![rule(&["echo"], true, RuleDecision::Allow)]).unwrap();
    assert_eq!(
        inner_only.evaluate(ShellGrammar::PosixSh, "sh -c echo"),
        RuleVerdict::Prompt
    );
    for command in [
        "powershell -enc ZQBjAGgAbwA=",
        "pwsh -c echo",
        "iex echo",
        "sh -x echo",
        "curl example | sh",
        "cmd /c powershell",
    ] {
        let evaluation = rules.evaluate_with_matches(ShellGrammar::PosixSh, command);
        assert_eq!(evaluation.verdict(), RuleVerdict::Prompt, "{command:?}");
        assert!(!evaluation.amendable, "{command:?}");
    }
    assert!(
        RuleSet::default()
            .evaluate_with_matches(ShellGrammar::PosixSh, "/usr/bin/git status")
            .amendable
    );
    assert!(
        !RuleSet::default()
            .evaluate_with_matches(ShellGrammar::CmdExe, r"C:\tools\powershell.exe -c echo")
            .amendable
    );
    let opaque_chain = RuleSet::default().evaluate_with_matches(
        ShellGrammar::CmdExe,
        "cmd /c \"powershell -Command bash -c echo\"",
    );
    assert_eq!(opaque_chain.verdict(), RuleVerdict::Prompt);
    assert!(!opaque_chain.amendable);
    let variable_inner =
        RuleSet::default().evaluate_with_matches(ShellGrammar::PosixSh, "sh -c '$INNER'");
    assert_eq!(variable_inner.verdict(), RuleVerdict::Prompt);
    assert!(!variable_inner.amendable);
}

#[test]
fn windows_suffixed_shells_get_bare_shell_recognition() {
    // `bash.exe -c` / `sh.exe -c` recurse into the inner layer exactly like
    // the bare forms.
    let inner_only = RuleSet::new(vec![rule(&["echo"], true, RuleDecision::Allow)]).unwrap();
    for (suffixed, bare) in [
        ("bash.exe -c echo", "bash -c echo"),
        ("sh.exe -c echo", "sh -c echo"),
    ] {
        let suffixed_eval = inner_only.evaluate_with_matches(ShellGrammar::PosixSh, suffixed);
        let bare_eval = inner_only.evaluate_with_matches(ShellGrammar::PosixSh, bare);
        assert_eq!(suffixed_eval.verdict(), bare_eval.verdict(), "{suffixed:?}");
        assert_eq!(suffixed_eval.verdict(), RuleVerdict::Prompt, "{suffixed:?}");
    }
    let full = RuleSet::new(vec![
        rule(&["bash.exe", "-c", "echo"], true, RuleDecision::Allow),
        rule(&["bash", "-c", "echo"], true, RuleDecision::Allow),
        rule(&["echo"], true, RuleDecision::Allow),
    ])
    .unwrap();
    assert_eq!(
        full.evaluate(ShellGrammar::PosixSh, "bash.exe -c echo"),
        RuleVerdict::Allow
    );
    // Inner-layer strictest-wins applies through the .exe name.
    let denying = RuleSet::new(vec![
        rule(&["bash.exe", "-c", "rm x"], true, RuleDecision::Allow),
        rule(&["rm"], false, RuleDecision::Deny),
    ])
    .unwrap();
    assert_eq!(
        denying.evaluate(ShellGrammar::PosixSh, "bash.exe -c 'rm x'"),
        RuleVerdict::Deny
    );
    // Every non-`-c` form is Opaque, never an ordinary amendable program.
    for command in [
        "bash.exe script.sh",
        "sh.exe script.sh",
        "bash.exe -x echo",
        "curl example | sh.exe",
        "pwsh.exe -c echo",
    ] {
        let evaluation = RuleSet::default().evaluate_with_matches(ShellGrammar::PosixSh, command);
        assert_eq!(evaluation.verdict(), RuleVerdict::Prompt, "{command:?}");
        assert!(!evaluation.amendable, "{command:?}");
        assert_eq!(
            evaluation.complex_reason,
            Some(ComplexReason::NestedOpaque),
            "{command:?}"
        );
    }
    // Pathed Windows-suffixed basenames are Opaque like their bare forms.
    for command in [r"C:\tools\bash.exe -c echo", r"C:\tools\sh.exe -i"] {
        let evaluation = RuleSet::default().evaluate_with_matches(ShellGrammar::CmdExe, command);
        assert_eq!(evaluation.verdict(), RuleVerdict::Prompt, "{command:?}");
        assert!(!evaluation.amendable, "{command:?}");
        assert_eq!(
            evaluation.complex_reason,
            Some(ComplexReason::NestedOpaque),
            "{command:?}"
        );
    }
}

#[test]
fn a_bash_exe_prefix_rule_cannot_auto_allow_arbitrary_scripts() {
    // An unanchored `["bash.exe"]` prefix rule (the shape a prefix amendment
    // would have minted before the fix) must not auto-allow anything: inner
    // analysis still runs and Opaque forms still prompt.
    let rules = RuleSet::new(vec![rule(&["bash.exe"], false, RuleDecision::Allow)]).unwrap();
    for command in [
        "bash.exe -c echo",
        "bash.exe -c 'rm x'",
        "bash.exe script.sh",
    ] {
        assert_eq!(
            rules.evaluate(ShellGrammar::PosixSh, command),
            RuleVerdict::Prompt,
            "{command:?}"
        );
    }
    let with_deny = RuleSet::new(vec![
        rule(&["bash.exe"], false, RuleDecision::Allow),
        rule(&["rm"], false, RuleDecision::Deny),
    ])
    .unwrap();
    assert_eq!(
        with_deny.evaluate(ShellGrammar::PosixSh, "bash.exe -c 'rm x'"),
        RuleVerdict::Deny
    );
    // And such an amendment can no longer be minted from a human-approved
    // `bash.exe` invocation in the first place (nested-layer refusal below).
    assert!(matches!(
        mint_amendment(
            "bash.exe -c echo",
            ShellGrammar::PosixSh,
            AmendmentKind::Prefix,
            None,
            added_at(),
        ),
        Err(RuleStoreError::CommandNotAmendable)
    ));
    assert!(matches!(
        mint_approval_rule(
            "bash.exe -c echo",
            ShellGrammar::PosixSh,
            ApprovalRuleChoice::AllowAlwaysExact,
            None,
            Some(added_at()),
        ),
        Err(RuleStoreError::CommandNotAmendable)
    ));
}

#[test]
fn minting_a_command_with_a_recognized_inner_layer_is_refused() {
    // A minted outer-segment rule alone can never authorize the approved
    // command (evaluation maps the unmatched inner layer to Prompt), so
    // minting refuses fail-closed instead of persisting a dead rule.
    for (command, shell) in [
        ("sh -c echo", ShellGrammar::PosixSh),
        ("bash -c echo", ShellGrammar::PosixSh),
        ("cmd /c dir", ShellGrammar::CmdExe),
    ] {
        let evaluation = RuleSet::default().evaluate_with_matches(shell, command);
        assert!(!evaluation.amendable, "{command:?}");
        assert!(
            matches!(
                mint_amendment(command, shell, AmendmentKind::Exact, None, added_at(),),
                Err(RuleStoreError::CommandNotAmendable)
            ),
            "{command:?}"
        );
    }
    // The refused mint persists nothing, so an identical later command still
    // prompts — no silent "always allow" that never applies.
    assert_eq!(
        RuleSet::default().evaluate(ShellGrammar::PosixSh, "sh -c echo"),
        RuleVerdict::Prompt
    );
    // Positive control: a flat clean command still mints and then allows.
    let minted = mint_amendment(
        "git status",
        ShellGrammar::PosixSh,
        AmendmentKind::Exact,
        None,
        added_at(),
    )
    .unwrap();
    let rules = RuleSet::new(minted.rules).unwrap();
    assert_eq!(
        rules.evaluate(ShellGrammar::PosixSh, "git status"),
        RuleVerdict::Allow
    );
}

#[test]
fn amendment_minting_is_exact_by_default_and_prefix_scope_is_explicit() {
    assert_eq!(
        ApprovalRuleChoice::default(),
        ApprovalRuleChoice::AllowAlwaysExact
    );
    assert_eq!(
        mint_approval_rule(
            "git status",
            ShellGrammar::PosixSh,
            ApprovalRuleChoice::AllowOnce,
            None,
            None,
        )
        .unwrap(),
        None
    );
    assert!(matches!(
        mint_amendment(
            "git status",
            ShellGrammar::PosixSh,
            AmendmentKind::Exact,
            None,
            String::new(),
        ),
        Err(RuleStoreError::RuleFileInvalid { .. })
    ));
    let exact = mint_amendment(
        "git push origin main",
        ShellGrammar::PosixSh,
        AmendmentKind::Exact,
        None,
        added_at(),
    )
    .unwrap();
    assert_eq!(exact.rules.len(), 1);
    assert!(exact.rules[0].exact);
    assert_eq!(exact.rules[0].decision, RuleDecision::Allow);
    assert_eq!(exact.rules[0].source, Some(RuleSource::ApprovalCard));
    assert!(
        exact.rules[0]
            .pattern
            .iter()
            .all(|token| matches!(token, PatternToken::Single(_)))
    );
    assert_eq!(
        exact.scope_text,
        "only this exact argv: git push origin main"
    );

    let prefix = mint_amendment(
        "git push origin main",
        ShellGrammar::PosixSh,
        AmendmentKind::Prefix,
        None,
        added_at(),
    )
    .unwrap();
    assert!(!prefix.rules[0].exact);
    assert_eq!(
        prefix.rules[0].pattern,
        vec![PatternToken::Single("git".into())]
    );
    assert_eq!(prefix.scope_text, "any future `git` command");

    let prompt_match = RuleMatch {
        rule_index: 7,
        matched_prefix: ["git", "push", "origin", "main"].map(String::from).to_vec(),
        decision: RuleDecision::Prompt,
    };
    let disclosed_prefix = mint_approval_rule(
        "git push origin main --force",
        ShellGrammar::PosixSh,
        ApprovalRuleChoice::AllowAlwaysPrefix,
        Some(&prompt_match),
        Some(added_at()),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        disclosed_prefix.rules[0].pattern,
        ["git", "push", "origin", "main"]
            .map(|token| PatternToken::Single(token.into()))
            .to_vec()
    );
    assert_eq!(
        disclosed_prefix.scope_text,
        "any future command whose argv starts with `git push origin main`, including with extra flags"
    );

    let compound = mint_amendment(
        "git status && cargo test",
        ShellGrammar::PosixSh,
        AmendmentKind::Exact,
        None,
        added_at(),
    )
    .unwrap();
    assert_eq!(compound.rules.len(), 2);
    assert_eq!(
        compound.scope_text,
        "only these exact argv segments: git status; cargo test"
    );

    assert!(
        mint_approval_rule(
            "git status",
            ShellGrammar::PosixSh,
            ApprovalRuleChoice::AllowAlwaysExact,
            None,
            Some(added_at()),
        )
        .unwrap()
        .is_some()
    );

    assert!(matches!(
        mint_amendment(
            "echo $HOME",
            ShellGrammar::PosixSh,
            AmendmentKind::Exact,
            None,
            added_at(),
        ),
        Err(RuleStoreError::CommandNotAmendable)
    ));
    assert!(matches!(
        mint_amendment(
            "powershell -enc ZQBjAGgAbwA=",
            ShellGrammar::PosixSh,
            AmendmentKind::Exact,
            None,
            added_at(),
        ),
        Err(RuleStoreError::CommandNotAmendable)
    ));
}

#[test]
fn rule_path_override_cannot_escape_nano_home() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let inside = home.join("provisioned");
    let outside = temp.path().join("outside");
    fs::create_dir_all(&inside).unwrap();
    fs::create_dir_all(&outside).unwrap();
    // rules_path returns canonicalized paths (dunce expands 8.3 short
    // names — CI profiles alias e.g. RUNNER~1), so compare canonical to
    // canonical; the escape denial below is the actual security content.
    let canon_home = dunce::canonicalize(&home).unwrap();
    let canon_inside = dunce::canonicalize(&inside).unwrap();
    assert_eq!(
        rules_path(&home, None).unwrap(),
        canon_home.join("rules.toml")
    );
    assert_eq!(
        rules_path(&home, Some(&inside.join("custom.toml"))).unwrap(),
        canon_inside.join("custom.toml")
    );
    assert!(matches!(
        rules_path(&home, Some(&outside.join("rules.toml"))),
        Err(RuleStoreError::RuleFileInvalid { .. })
    ));
}

#[test]
fn configured_rules_path_uses_the_environment_override() {
    const CHILD: &str = "NANO_RULES_ENV_TEST_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let home = std::path::PathBuf::from(std::env::var_os("NANO_TEST_HOME").unwrap());
        // configured_rules_path canonicalizes; compare against the
        // canonicalized expectation (CI profiles use 8.3 aliases). The
        // file itself need not exist — canonicalize the parent + join.
        let raw = std::path::PathBuf::from(std::env::var_os("NANO_TEST_RULES").unwrap());
        let expected = dunce::canonicalize(raw.parent().unwrap())
            .unwrap()
            .join(raw.file_name().unwrap());
        assert_eq!(configured_rules_path(&home).unwrap(), expected);
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let provisioned = home.join("provisioned");
    fs::create_dir_all(&provisioned).unwrap();
    let expected = provisioned.join("operator.toml");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "configured_rules_path_uses_the_environment_override",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("NANO_TEST_HOME", &home)
        .env("NANO_TEST_RULES", &expected)
        .env("NANO_RULES_FILE", &expected)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "environment child failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn malformed_partial_unknown_and_duplicate_files_fail_closed() {
    for contents in [
        "not toml",
        r#"[[rule]]
pattern = ["git"]
decision = "allow"
exact = true

[[rule]]
pattern = []
decision = "allow"
exact = true
"#,
        r#"[[rule]]
pattern = ["git"]
decision = "allow"
exact = true
surprise = true
"#,
        r#"[[rule]]
pattern = ["git"]
decision = "allow"
exact = true
exact = false
"#,
    ] {
        let temp = tempfile::tempdir().unwrap();
        write_owner_only(&temp.path().join("rules.toml"), contents);
        assert!(matches!(
            load_rules(temp.path(), None),
            Err(RuleStoreError::RuleFileInvalid { .. })
        ));
    }
}

#[cfg(unix)]
#[test]
fn owner_only_valid_file_loads_but_open_permissions_fail_closed() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("rules.toml");
    write_owner_only(
        &path,
        r#"[[rule]]
pattern = ["git", ["status", "diff"]]
decision = "allow"
exact = false
"#,
    );
    assert_eq!(load_rules(temp.path(), None).unwrap().rules().len(), 1);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o606)).unwrap();
    assert!(matches!(
        load_rules(temp.path(), None),
        Err(RuleStoreError::RuleFileInvalid { .. })
    ));
}

/// F-P4-2: the interim "ACL helper unavailable" refusal is gone — the
/// ported P2a §5.5 DACL audit now backs the Windows load path. A file
/// written by the amendment writer (explicit current-user-only DACL) loads;
/// a hand-widened DACL fails closed (covered by the in-module unit test
/// `windows_acl_audit_loads_pinned_file_and_refuses_widened_dacl`).
#[cfg(windows)]
#[test]
fn windows_acl_audit_loads_amendment_written_rules_file() {
    let temp = tempfile::tempdir().unwrap();
    let amendment = mint_amendment(
        "git status",
        ShellGrammar::CmdExe,
        AmendmentKind::Exact,
        None,
        added_at(),
    )
    .unwrap();
    append_amendment(temp.path(), None, &amendment).unwrap();
    let loaded = load_rules(temp.path(), None).unwrap();
    assert_eq!(loaded.rules().len(), 1);
    assert_eq!(
        loaded.evaluate(ShellGrammar::CmdExe, "git status"),
        RuleVerdict::Allow
    );
}

/// F-P4-2: amendments write on Windows now (the stub refused before any
/// I/O); the sidecar lock is released afterward and the file reloads.
#[cfg(windows)]
#[test]
fn windows_amendment_writes_reloads_and_releases_the_lock() {
    let temp = tempfile::tempdir().unwrap();
    let amendment = mint_amendment(
        "git status",
        ShellGrammar::CmdExe,
        AmendmentKind::Exact,
        None,
        added_at(),
    )
    .unwrap();
    let loaded = append_amendment(temp.path(), None, &amendment).unwrap();
    assert_eq!(loaded.rules().len(), 1);
    assert!(temp.path().join("rules.toml").exists());
    // The lock is a sidecar that survives as a file but must be RELEASABLE:
    // a second amendment succeeds (a held lock would be LockBusy).
    let second = mint_amendment(
        "cargo test",
        ShellGrammar::CmdExe,
        AmendmentKind::Exact,
        None,
        added_at(),
    )
    .unwrap();
    let loaded = append_amendment(temp.path(), None, &second).unwrap();
    assert_eq!(loaded.rules().len(), 2);
}

#[cfg(unix)]
#[test]
fn append_is_atomic_owner_only_and_refuses_an_invalid_base() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let first = mint_amendment(
        "git status",
        ShellGrammar::PosixSh,
        AmendmentKind::Exact,
        None,
        added_at(),
    )
    .unwrap();
    let loaded = append_amendment(temp.path(), None, &first).unwrap();
    assert_eq!(loaded.rules().len(), 1);
    let path = temp.path().join("rules.toml");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::write(&path, "invalid = true\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(matches!(
        append_amendment(temp.path(), None, &first),
        Err(RuleStoreError::RuleFileInvalid { .. })
    ));
    assert_eq!(fs::read(&path).unwrap(), before);
}

// Runs on Windows too since F-P4-2: the sidecar lock + ACL-pinned amendment
// writer are platform-covered; this is the two-writer lost-update leg.
#[cfg(any(unix, windows))]
#[test]
fn concurrent_amendments_preserve_the_union() {
    let temp = tempfile::tempdir().unwrap();
    let executable = std::env::current_exe().unwrap();
    let children =
        [("git status", "first"), ("cargo test", "second")].map(|(command, child_id)| {
            std::process::Command::new(&executable)
                .args([
                    "--exact",
                    "concurrent_amendment_child_process",
                    "--nocapture",
                ])
                .env("NANO_RULE_APPEND_CHILD_HOME", temp.path())
                .env("NANO_RULE_APPEND_CHILD_COMMAND", command)
                .env("NANO_RULE_APPEND_CHILD_ID", child_id)
                .spawn()
                .unwrap()
        });
    let outputs = children.map(|child| child.wait_with_output().unwrap());
    for output in outputs {
        assert!(
            output.status.success(),
            "writer child failed:\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let final_rules = load_rules(temp.path(), None).unwrap();
    assert_eq!(final_rules.rules().len(), 2);
    assert_eq!(
        final_rules.evaluate(ShellGrammar::PosixSh, "git status"),
        RuleVerdict::Allow
    );
    assert_eq!(
        final_rules.evaluate(ShellGrammar::PosixSh, "cargo test"),
        RuleVerdict::Allow
    );
}

#[cfg(any(unix, windows))]
#[test]
fn concurrent_amendment_child_process() {
    let Some(home) = std::env::var_os("NANO_RULE_APPEND_CHILD_HOME") else {
        return;
    };
    let command = std::env::var("NANO_RULE_APPEND_CHILD_COMMAND").unwrap();
    let child_id = std::env::var("NANO_RULE_APPEND_CHILD_ID").unwrap();
    let amendment = mint_amendment(
        &command,
        ShellGrammar::PosixSh,
        AmendmentKind::Exact,
        None,
        added_at(),
    )
    .unwrap();
    let home = std::path::PathBuf::from(home);
    for _ in 0..1_000 {
        match append_amendment(&home, None, &amendment) {
            Ok(_) => {
                fs::write(home.join(format!("writer-{child_id}.done")), b"done").unwrap();
                return;
            }
            Err(RuleStoreError::LockBusy) => {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(error) => panic!("unexpected append failure: {error}"),
        }
    }
    panic!("lock remained busy after bounded retries");
}

#[cfg(unix)]
#[test]
fn symlink_rules_file_is_refused() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target.toml");
    write_owner_only(&target, "");
    symlink(&target, temp.path().join("rules.toml")).unwrap();
    assert!(matches!(
        load_rules(temp.path(), None),
        Err(RuleStoreError::RuleFileInvalid { .. })
    ));
}

fn write_owner_only(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn added_at() -> String {
    "2026-08-13T00:00:00Z".into()
}
