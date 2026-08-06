use fns_fs::{
    DEFAULT_EXCLUDES, DEFAULT_INCLUDES, FsError, HARD_INTERNAL_EXCLUDES, HARD_SECRET_EXCLUDES,
    RuleSource, SAFE_ENV_BASENAMES, SyncRuleConfig, SyncRules,
};
use fns_protocol::WorkspacePath;

fn path(value: &str) -> WorkspacePath {
    WorkspacePath::parse(value).expect("test path is valid")
}

#[test]
fn fixed_rule_precedence_and_safe_env_exceptions() {
    let rules = SyncRules::compile(SyncRuleConfig {
        includes: vec!["target/keep/**".into(), ".env".into()],
        excludes: vec!["src/private/**".into()],
        protect_secrets: true,
    })
    .unwrap();

    assert_eq!(
        rules.decide(&path(".env"), false).source,
        RuleSource::HardSecurity
    );
    assert!(!rules.decide(&path(".env"), false).included);
    assert_eq!(
        rules.decide(&path(".env.example"), false).source,
        RuleSource::UserInclude
    );
    assert!(rules.decide(&path(".env.example"), false).included);
    assert_eq!(
        rules.decide(&path("src/private/key.txt"), false).source,
        RuleSource::UserExclude
    );
    assert!(!rules.decide(&path("src/private/key.txt"), false).included);
    assert_eq!(
        rules.decide(&path("target/keep/a.o"), false).source,
        RuleSource::UserInclude
    );
    assert!(rules.decide(&path("target/keep/a.o"), false).included);
    assert_eq!(
        rules.decide(&path("target/drop/a.o"), false).source,
        RuleSource::DefaultExclude
    );
    assert!(!rules.decide(&path("target/drop/a.o"), false).included);
    assert!(rules.should_descend(&path("target")));
}

#[test]
fn advanced_secret_disable_does_not_disable_internal_exclusions() {
    let rules = SyncRules::compile(SyncRuleConfig {
        includes: vec!["**".into()],
        excludes: Vec::new(),
        protect_secrets: false,
    })
    .unwrap();

    assert!(rules.decide(&path(".env"), false).included);
    assert!(!rules.decide(&path(".fns-internal/blob"), false).included);
    assert_eq!(
        rules.decide(&path(".fns-internal/blob"), false).source,
        RuleSource::HardInternal
    );
}

#[test]
fn locked_rule_constants_are_exposed_verbatim() {
    assert_eq!(DEFAULT_INCLUDES, &["**"]);
    assert_eq!(
        DEFAULT_EXCLUDES,
        &[
            ".git/**",
            ".hg/**",
            ".svn/**",
            "node_modules/**",
            ".venv/**",
            "venv/**",
            "target/**",
            "build/**",
            "dist/**",
            ".next/**",
            ".cache/**",
            "**/__pycache__/**",
            "**/.pytest_cache/**",
            "**/.mypy_cache/**",
            "**/.ruff_cache/**",
            "**/*.log",
            "**/*.sock",
            "**/*.pid",
            "**/.DS_Store",
            "**/.fns_state.json",
        ]
    );
    assert_eq!(
        HARD_INTERNAL_EXCLUDES,
        &[
            ".fns_state.json",
            "**/.fns_state.json",
            ".fns-tmp-*",
            "**/.fns-tmp-*",
            ".fns-delete-*",
            "**/.fns-delete-*",
            ".fns-internal/**",
            "**/.fns-internal/**",
        ]
    );
    assert_eq!(
        HARD_SECRET_EXCLUDES,
        &[
            ".env",
            "**/.env",
            ".env.*",
            "**/.env.*",
            "*.pem",
            "**/*.pem",
            "*.key",
            "**/*.key",
            ".ssh/**",
            "**/.ssh/**",
            "id_rsa",
            "**/id_rsa",
            "id_ed25519",
            "**/id_ed25519",
            ".aws/credentials",
            "**/.aws/credentials",
            ".config/gcloud/application_default_credentials.json",
            "**/.config/gcloud/application_default_credentials.json",
            ".azure/accessTokens.json",
            "**/.azure/accessTokens.json",
            ".azure/msal_token_cache.json",
            "**/.azure/msal_token_cache.json",
            ".kube/config",
            "**/.kube/config",
            ".docker/config.json",
            "**/.docker/config.json",
        ]
    );
    assert_eq!(
        SAFE_ENV_BASENAMES,
        &[".env.example", ".env.sample", ".env.template"]
    );
}

#[test]
fn default_configuration_uses_default_rule_sources() {
    let rules = SyncRules::compile(SyncRuleConfig::default()).unwrap();

    assert_eq!(
        rules.decide(&path("notes/today.md"), false).source,
        RuleSource::DefaultInclude
    );
    assert_eq!(
        rules.decide(&path("target/debug/app"), false).source,
        RuleSource::DefaultExclude
    );
    assert_eq!(
        rules.decide(&path(".env.local"), false).source,
        RuleSource::HardSecurity
    );
}

#[test]
fn default_exclusions_match_with_hard_internal_precedence() {
    let rules = SyncRules::compile(SyncRuleConfig::default()).unwrap();

    for value in [
        ".git/config",
        ".hg/store",
        ".svn/entries",
        "node_modules/pkg/index.js",
        ".venv/bin/python",
        "venv/bin/python",
        "target/debug/app",
        "build/app",
        "dist/app",
        ".next/cache/page",
        ".cache/item",
        "src/__pycache__/module.pyc",
        "src/.pytest_cache/item",
        "src/.mypy_cache/item",
        "src/.ruff_cache/item",
        "logs/app.log",
        "run/app.sock",
        "run/app.pid",
        "docs/.DS_Store",
    ] {
        let decision = rules.decide(&path(value), false);
        assert_eq!(decision.source, RuleSource::DefaultExclude, "{value}");
        assert!(!decision.included, "{value}");
    }

    let state_file = rules.decide(&path("state/.fns_state.json"), false);
    assert_eq!(state_file.source, RuleSource::HardInternal);
    assert!(!state_file.included);
}

#[test]
fn every_hard_internal_exclusion_matches_at_root_and_when_nested() {
    let rules = SyncRules::compile(SyncRuleConfig {
        includes: vec!["**".into()],
        excludes: Vec::new(),
        protect_secrets: false,
    })
    .unwrap();

    for value in [
        ".fns_state.json",
        "state/.fns_state.json",
        ".fns-tmp-upload",
        "state/.fns-tmp-upload",
        ".fns-delete-old",
        "state/.fns-delete-old",
        ".fns-internal/blob",
        "state/.fns-internal/blob",
    ] {
        let decision = rules.decide(&path(value), false);
        assert_eq!(decision.source, RuleSource::HardInternal, "{value}");
        assert!(!decision.included, "{value}");
    }
}

#[test]
fn every_hard_secret_family_matches_at_root_and_when_nested() {
    let rules = SyncRules::compile(SyncRuleConfig::default()).unwrap();

    for value in [
        ".env",
        "nested/.env",
        ".env.local",
        "nested/.env.local",
        "certificate.pem",
        "nested/certificate.pem",
        "private.key",
        "nested/private.key",
        ".ssh/config",
        "nested/.ssh/config",
        "id_rsa",
        "nested/id_rsa",
        "id_ed25519",
        "nested/id_ed25519",
        ".aws/credentials",
        "nested/.aws/credentials",
        ".config/gcloud/application_default_credentials.json",
        "nested/.config/gcloud/application_default_credentials.json",
        ".azure/accessTokens.json",
        "nested/.azure/accessTokens.json",
        ".azure/msal_token_cache.json",
        "nested/.azure/msal_token_cache.json",
        ".kube/config",
        "nested/.kube/config",
        ".docker/config.json",
        "nested/.docker/config.json",
    ] {
        let decision = rules.decide(&path(value), false);
        assert_eq!(decision.source, RuleSource::HardSecurity, "{value}");
        assert!(!decision.included, "{value}");
    }
}

#[test]
fn safe_env_basenames_are_not_treated_as_secrets_at_any_depth() {
    let rules = SyncRules::compile(SyncRuleConfig::default()).unwrap();

    for value in [
        ".env.example",
        ".env.sample",
        ".env.template",
        "nested/.env.example",
        "nested/.env.sample",
        "nested/.env.template",
    ] {
        let decision = rules.decide(&path(value), false);
        assert_eq!(decision.source, RuleSource::DefaultInclude, "{value}");
        assert!(decision.included, "{value}");
    }
}

#[test]
fn explicit_env_includes_apply_to_safe_variants_at_the_same_depth() {
    let rules = SyncRules::compile(SyncRuleConfig {
        includes: vec![".env".into(), "nested/.env".into()],
        excludes: Vec::new(),
        protect_secrets: true,
    })
    .unwrap();

    for value in [
        ".env.example",
        ".env.sample",
        ".env.template",
        "nested/.env.example",
        "nested/.env.sample",
        "nested/.env.template",
    ] {
        let decision = rules.decide(&path(value), false);
        assert_eq!(decision.source, RuleSource::UserInclude, "{value}");
        assert!(decision.included, "{value}");
    }
}

#[test]
fn explicit_env_excludes_win_for_safe_variants() {
    let rules = SyncRules::compile(SyncRuleConfig {
        includes: vec![".env".into(), "nested/.env".into()],
        excludes: vec![".env".into(), "nested/.env".into()],
        protect_secrets: true,
    })
    .unwrap();

    for value in [".env.example", "nested/.env.template"] {
        let decision = rules.decide(&path(value), false);
        assert_eq!(decision.source, RuleSource::UserExclude, "{value}");
        assert!(!decision.included, "{value}");
    }
}

#[test]
fn invalid_user_globs_return_sanitized_rule_errors() {
    let result = SyncRules::compile(SyncRuleConfig {
        includes: vec!["notes/**".into(), "/Users/alice/secret/[".into()],
        excludes: Vec::new(),
        protect_secrets: true,
    });
    let error = match result {
        Ok(_) => panic!("invalid glob unexpectedly compiled"),
        Err(error) => error,
    };

    match error {
        FsError::InvalidRule { index, reason } => {
            assert_eq!(index, 1);
            assert!(!reason.contains("/Users/alice/secret"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn user_globs_use_literal_path_separators() {
    let rules = SyncRules::compile(SyncRuleConfig {
        includes: vec!["target/*".into()],
        excludes: Vec::new(),
        protect_secrets: true,
    })
    .unwrap();

    assert_eq!(
        rules.decide(&path("target/one"), false).source,
        RuleSource::UserInclude
    );
    assert_eq!(
        rules.decide(&path("target/one/two"), false).source,
        RuleSource::DefaultExclude
    );
}

#[test]
fn backslashes_are_not_glob_escapes() {
    SyncRules::compile(SyncRuleConfig {
        includes: Vec::new(),
        excludes: vec![r"trailing\".into()],
        protect_secrets: true,
    })
    .expect("a trailing backslash is literal when escaping is disabled");
}

#[test]
fn descent_never_crosses_hard_or_user_exclusions() {
    let hard_rules = SyncRules::compile(SyncRuleConfig {
        includes: vec![".fns-internal/keep/**".into(), ".ssh/keep/**".into()],
        excludes: Vec::new(),
        protect_secrets: true,
    })
    .unwrap();
    assert!(!hard_rules.should_descend(&path(".fns-internal")));
    assert!(!hard_rules.should_descend(&path(".ssh")));

    let user_rules = SyncRules::compile(SyncRuleConfig {
        includes: vec!["target/keep/**".into()],
        excludes: vec!["target/**".into()],
        protect_secrets: true,
    })
    .unwrap();
    assert!(!user_rules.should_descend(&path("target")));
}

#[test]
fn default_excluded_directories_need_a_user_include_ancestor_hint() {
    let default_rules = SyncRules::compile(SyncRuleConfig::default()).unwrap();
    assert!(!default_rules.should_descend(&path("target")));

    let included_rules = SyncRules::compile(SyncRuleConfig {
        includes: vec!["target/keep/**".into()],
        excludes: Vec::new(),
        protect_secrets: true,
    })
    .unwrap();
    assert!(included_rules.should_descend(&path("target")));
}
