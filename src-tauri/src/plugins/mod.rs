pub mod compat;
pub mod declarative;
pub mod egress;
pub mod ingress;
pub mod installer;
pub mod manifest;
pub mod registry;
pub mod subprocess;

pub use compat::*;
pub use declarative::*;
pub use egress::*;
pub use ingress::*;
pub use installer::*;
pub use manifest::*;
pub use registry::*;
pub use subprocess::*;

#[cfg(test)]
mod e2e_tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn reference_qoder_ingress_and_codex_egress_plugins_validate_and_match_compatibly() {
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let qoder_dir = repo_root.join("examples/plugins/ingress-qoder");
        let codex_dir = repo_root.join("examples/plugins/egress-codex-sub");

        let (qoder_manifest, qoder_hash) = validate_plugin_dir(&qoder_dir).unwrap();
        let (codex_manifest, codex_hash) = validate_plugin_dir(&codex_dir).unwrap();

        let mut reg = PluginRegistry::new_with_builtins();
        reg.register_external(qoder_manifest.clone(), qoder_hash, qoder_dir, true)
            .unwrap();
        reg.register_external(codex_manifest.clone(), codex_hash, codex_dir, true)
            .unwrap();

        let ingress_prof = reg.ingress_profile("qoder_ingress").unwrap();
        let egress_prof = reg.egress_profile("codex_sub").unwrap();
        let report = CompatibilityEngine::evaluate(
            ingress_prof,
            egress_prof,
            None,
            &BridgeMatrix::default_kernel(),
        );
        assert_eq!(report.level, CompatibilityLevel::Degraded);
        assert!(!report
            .issues
            .iter()
            .any(|i| i.severity == IssueSeverity::Blocker));
        assert!(report.auto_shapers.contains(&"must_stream".to_string()));
        assert!(report
            .auto_shapers
            .contains(&"requires_instructions".to_string()));
        assert!(report.auto_shapers.contains(&"disallow_store".to_string()));

        // 也验证从 JSON 单文件导入声明式 qoder 插件
        let tmp = std::env::temp_dir().join(format!("qwitch_e2e_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let staged = stage_json_plugin(
            &repo_root.join("examples/plugins/ingress-qoder/manifest.json"),
            &tmp,
            &HashSet::new(),
        )
        .unwrap();
        let committed = commit_staged_plugin(staged, &tmp).unwrap();
        assert!(committed.join("manifest.json").is_file());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
