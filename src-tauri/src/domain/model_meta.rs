//! 模型元数据（上下文窗口 / 最大输出）。
//!
//! 模型目录此前只有 id 和显示名，UI 与网关都不知道「这个模型能吃多长的上下文」。
//! 这里内嵌一份 models.dev 快照（`resources/model_meta.json`），在
//! `normalize_upstream_model` 这个唯一构造入口处补齐——实时探测、本机客户端目录、
//! 内置预设、静态兜底全部受益，不需要各自查表。
//!
//! 原则：**只补不猜**。id 未命中就留空，UI 自然不显示，绝不编造数字。

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize)]
pub struct ModelMeta {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub context: u64,
    #[serde(default)]
    pub output: Option<u64>,
}

#[derive(Deserialize)]
struct MetaFile {
    #[serde(default)]
    models: HashMap<String, ModelMeta>,
}

fn table() -> &'static HashMap<String, ModelMeta> {
    static TABLE: OnceLock<HashMap<String, ModelMeta>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let parsed: MetaFile = serde_json::from_str(include_str!(
            "../../resources/model_meta.json"
        ))
        .unwrap_or(MetaFile {
            models: HashMap::new(),
        });
        // 大小写不敏感：不同上游对同一个模型的 id 大小写并不统一
        parsed
            .models
            .into_iter()
            .map(|(id, meta)| (id.to_ascii_lowercase(), meta))
            .collect()
    })
}

/// 按模型 id 查元数据；未命中返回 None（调用方保持字段为空）。
pub fn lookup(model_id: &str) -> Option<&'static ModelMeta> {
    table().get(&model_id.trim().to_ascii_lowercase())
}

/// 已知模型数量（用于诊断与测试断言）。
pub fn known_model_count() -> usize {
    table().len()
}

// ---------------------------------------------------------------------------
// 内置推荐目录（preset）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct PresetEntry {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub context: Option<u64>,
    #[serde(default)]
    pub output: Option<u64>,
}

#[derive(Deserialize)]
struct PresetFile {
    #[serde(default)]
    presets: HashMap<String, Vec<PresetEntry>>,
}

fn presets() -> &'static HashMap<String, Vec<PresetEntry>> {
    static P: OnceLock<HashMap<String, Vec<PresetEntry>>> = OnceLock::new();
    P.get_or_init(|| {
        serde_json::from_str::<PresetFile>(include_str!("../../resources/model_presets.json"))
            .map(|f| f.presets)
            .unwrap_or_default()
    })
}

/// 该工具的内置推荐目录（原始定义，未归一化）；没有则返回空切片。
///
/// 放在 JSON 而不是代码里，是因为这些是**需要随上游更新**的数据：
/// 加一个模型、纠正一个输出上限，不应该触发一次 Rust 编译。
pub fn preset_entries(tool_id: &str) -> &'static [PresetEntry] {
    presets().get(tool_id).map(Vec::as_slice).unwrap_or(&[])
}
