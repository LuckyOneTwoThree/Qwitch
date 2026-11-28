use std::collections::HashSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

use crate::adapter::external::{adapter_content_hash, is_valid_entry_args, is_valid_entry_command};
use crate::plugins::manifest::{is_valid_plugin_id, PluginManifest, PluginTransport};

pub const MAX_PLUGIN_FILES: usize = 500;
pub const MAX_PLUGIN_TOTAL_BYTES: u64 = 32 * 1024 * 1024; // 32 MiB
pub const MAX_PLUGIN_SINGLE_FILE_BYTES: u64 = 16 * 1024 * 1024; // 16 MiB
pub const MAX_PLUGIN_DEPTH: usize = 4;
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone)]
pub struct StagedPlugin {
    pub manifest: PluginManifest,
    pub staging_dir: PathBuf,
    pub content_hash: String,
    pub overrides_builtin: bool,
}

pub fn plugins_root_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("plugins")
}

pub fn validate_plugin_dir(dir: &Path) -> Result<(PluginManifest, String), String> {
    let manifest_path = dir.join("manifest.json");
    let meta = std::fs::symlink_metadata(&manifest_path)
        .map_err(|_| "插件目录缺少 manifest.json".to_string())?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > MAX_MANIFEST_BYTES {
        return Err("manifest.json 无效、为符号链接或体积超过 256KB".into());
    }
    let bytes =
        std::fs::read(&manifest_path).map_err(|e| format!("读取 manifest.json 失败: {e}"))?;
    let manifest: PluginManifest =
        serde_json::from_slice(&bytes).map_err(|e| format!("解析 manifest.json 失败: {e}"))?;

    if !is_valid_plugin_id(&manifest.id) {
        return Err(format!("非法插件 ID: {}", manifest.id));
    }

    if manifest.transport == PluginTransport::Subprocess {
        let entry = manifest
            .entry
            .as_ref()
            .ok_or_else(|| "subprocess 类型插件必须声明 entry".to_string())?;
        if !is_valid_entry_command(dir, &entry.command)
            || !is_valid_entry_args(dir, &entry.command, &entry.args)
        {
            return Err(format!(
                "插件 {} 的入口命令或参数未通过安全校验",
                manifest.id
            ));
        }
    }

    let hash = adapter_content_hash(dir)?;
    Ok((manifest, hash))
}

fn create_staging_dir(data_dir: &Path) -> Result<PathBuf, String> {
    let root = plugins_root_dir(data_dir);
    std::fs::create_dir_all(&root).map_err(|e| format!("创建插件目录失败: {e}"))?;
    let staging = root.join(format!(".staging-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&staging).map_err(|e| format!("创建临时暂存目录失败: {e}"))?;
    Ok(staging)
}

fn validate_relative_entry_path(rel: &Path) -> Result<usize, String> {
    let mut depth = 0usize;
    for comp in rel.components() {
        match comp {
            Component::Normal(seg) => {
                let s = seg.to_string_lossy();
                if s.chars().any(|c| c.is_control()) {
                    return Err("压缩包路径包含控制字符".into());
                }
                depth += 1;
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("检测到非法路径穿越: {}", rel.display()));
            }
        }
    }
    if depth == 0 {
        return Err("空路径条目".into());
    }
    if depth > MAX_PLUGIN_DEPTH + 1 {
        return Err("插件目录层级过深（超过 4 层）".into());
    }
    Ok(depth)
}

fn maybe_unwrap_single_root(staging_dir: &Path) -> Result<(), String> {
    if staging_dir.join("manifest.json").is_file() {
        return Ok(());
    }
    let entries: Vec<PathBuf> = std::fs::read_dir(staging_dir)
        .map_err(|e| format!("读取暂存目录失败: {e}"))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n != "__MACOSX" && !n.starts_with('.'))
        })
        .collect();

    if entries.len() == 1 && entries[0].is_dir() && entries[0].join("manifest.json").is_file() {
        let inner = &entries[0];
        let tmp_inner = staging_dir.join(format!(".unwrap-{}", Uuid::new_v4()));
        std::fs::rename(inner, &tmp_inner).map_err(|e| format!("剥离外层目录失败: {e}"))?;
        for child in std::fs::read_dir(&tmp_inner).map_err(|e| format!("读取内层目录失败: {e}"))?
        {
            let child = child.map_err(|e| format!("遍历内层目录失败: {e}"))?;
            let target = staging_dir.join(child.file_name());
            std::fs::rename(child.path(), target).map_err(|e| format!("提升内层文件失败: {e}"))?;
        }
        let _ = std::fs::remove_dir_all(&tmp_inner);
    }
    Ok(())
}

pub fn stage_zip_plugin(
    zip_path: &Path,
    data_dir: &Path,
    builtin_ids: &HashSet<String>,
) -> Result<StagedPlugin, String> {
    let staging_dir = create_staging_dir(data_dir)?;
    let res = (|| -> Result<StagedPlugin, String> {
        let file = std::fs::File::open(zip_path)
            .map_err(|e| format!("打开 ZIP 文件失败 {}: {e}", zip_path.display()))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| format!("无效的 ZIP 压缩包: {e}"))?;

        let mut file_count = 0usize;
        let mut total_bytes = 0u64;

        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|e| format!("读取 ZIP 条目失败: {e}"))?;

            // 防线 1：Zip Slip 路径穿越防护
            let Some(rel_path) = entry.enclosed_name() else {
                return Err(format!(
                    "ZIP 包含非法穿越路径: {}",
                    entry
                        .name()
                        .unwrap_or(std::borrow::Cow::Borrowed("<invalid>"))
                ));
            };
            // 跳过 macOS 资源派生目录
            if rel_path
                .components()
                .next()
                .is_some_and(|c| c.as_os_str() == "__MACOSX")
            {
                continue;
            }
            validate_relative_entry_path(&rel_path)?;

            // 防线 2：拒绝符号链接（Unix S_IFLNK = 0o120000）
            if entry
                .unix_mode()
                .is_some_and(|mode| (mode & 0o170000) == 0o120000)
            {
                return Err(format!("ZIP 不允许包含符号链接: {}", rel_path.display()));
            }

            let dest = staging_dir.join(&rel_path);
            if entry.is_dir() {
                std::fs::create_dir_all(&dest)
                    .map_err(|e| format!("创建目录失败 {}: {e}", dest.display()))?;
                continue;
            }

            // 防线 3：文件数与解压配额限制（流式限流读取，不轻信头部声明大小）
            file_count += 1;
            if file_count > MAX_PLUGIN_FILES {
                return Err(format!("ZIP 文件数超过 {MAX_PLUGIN_FILES} 上限"));
            }
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("创建父目录失败 {}: {e}", parent.display()))?;
            }

            let mut out_file = std::fs::File::create(&dest)
                .map_err(|e| format!("写入解压文件失败 {}: {e}", dest.display()))?;
            let mut limited = (&mut entry).take(MAX_PLUGIN_SINGLE_FILE_BYTES + 1);
            let copied = std::io::copy(&mut limited, &mut out_file)
                .map_err(|e| format!("解压文件内容失败 {}: {e}", rel_path.display()))?;
            if copied > MAX_PLUGIN_SINGLE_FILE_BYTES {
                return Err(format!("单文件体积超过 16MB 上限: {}", rel_path.display()));
            }
            total_bytes = total_bytes
                .checked_add(copied)
                .ok_or_else(|| "ZIP 解压体积溢出".to_string())?;
            if total_bytes > MAX_PLUGIN_TOTAL_BYTES {
                return Err("ZIP 解压总大小超过 32MB 上限".into());
            }
        }

        // 防线 4：智能单层根目录剥离
        maybe_unwrap_single_root(&staging_dir)?;

        // 防线 5：完整清单与哈希校验
        let (manifest, content_hash) = validate_plugin_dir(&staging_dir)?;
        let overrides_builtin = builtin_ids.contains(&manifest.id);
        Ok(StagedPlugin {
            manifest,
            staging_dir: staging_dir.clone(),
            content_hash,
            overrides_builtin,
        })
    })();

    if res.is_err() {
        let _ = std::fs::remove_dir_all(&staging_dir);
    }
    res
}

pub fn stage_json_plugin(
    json_path: &Path,
    data_dir: &Path,
    builtin_ids: &HashSet<String>,
) -> Result<StagedPlugin, String> {
    let meta =
        std::fs::symlink_metadata(json_path).map_err(|e| format!("读取插件文件失败: {e}"))?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > MAX_MANIFEST_BYTES {
        return Err("单文件插件无效、为符号链接或体积超过 256KB".into());
    }
    let bytes = std::fs::read(json_path).map_err(|e| format!("读取 JSON 失败: {e}"))?;
    let manifest: PluginManifest =
        serde_json::from_slice(&bytes).map_err(|e| format!("解析插件 JSON 失败: {e}"))?;
    if manifest.transport != PluginTransport::Declarative {
        return Err(
            "单文件 JSON 仅支持 declarative 纯声明式插件；可执行插件请打包为 .zip 或目录导入"
                .into(),
        );
    }

    let staging_dir = create_staging_dir(data_dir)?;
    let res = (|| -> Result<StagedPlugin, String> {
        std::fs::write(staging_dir.join("manifest.json"), &bytes)
            .map_err(|e| format!("写入暂存 manifest.json 失败: {e}"))?;
        let (validated, content_hash) = validate_plugin_dir(&staging_dir)?;
        let overrides_builtin = builtin_ids.contains(&validated.id);
        Ok(StagedPlugin {
            manifest: validated,
            staging_dir: staging_dir.clone(),
            content_hash,
            overrides_builtin,
        })
    })();

    if res.is_err() {
        let _ = std::fs::remove_dir_all(&staging_dir);
    }
    res
}

fn copy_dir_bounded(
    src: &Path,
    dst: &Path,
    depth: usize,
    file_count: &mut usize,
    total_bytes: &mut u64,
) -> Result<(), String> {
    if depth > MAX_PLUGIN_DEPTH {
        return Err("插件目录层级过深（超过 4 层）".into());
    }
    std::fs::create_dir_all(dst).map_err(|e| format!("创建目标目录失败: {e}"))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("读取插件目录失败: {e}"))? {
        let entry = entry.map_err(|e| format!("遍历插件目录失败: {e}"))?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|e| format!("读取元数据失败 {}: {e}", path.display()))?;
        if meta.file_type().is_symlink() {
            return Err(format!(
                "外部插件目录不允许包含符号链接: {}",
                path.display()
            ));
        }
        let target = dst.join(entry.file_name());
        if meta.is_dir() {
            copy_dir_bounded(&path, &target, depth + 1, file_count, total_bytes)?;
        } else if meta.is_file() {
            *file_count += 1;
            if *file_count > MAX_PLUGIN_FILES {
                return Err(format!("插件文件数量超过 {MAX_PLUGIN_FILES} 上限"));
            }
            let len = meta.len();
            if len > MAX_PLUGIN_SINGLE_FILE_BYTES {
                return Err(format!("单文件超过 16MB 上限: {}", path.display()));
            }
            *total_bytes = total_bytes
                .checked_add(len)
                .ok_or_else(|| "插件总大小溢出".to_string())?;
            if *total_bytes > MAX_PLUGIN_TOTAL_BYTES {
                return Err("插件目录总大小超过 32MB 上限".into());
            }
            std::fs::copy(&path, &target)
                .map_err(|e| format!("复制文件失败 {}: {e}", path.display()))?;
        }
    }
    Ok(())
}

pub fn stage_dir_plugin(
    src_dir: &Path,
    data_dir: &Path,
    builtin_ids: &HashSet<String>,
) -> Result<StagedPlugin, String> {
    let staging_dir = create_staging_dir(data_dir)?;
    let res = (|| -> Result<StagedPlugin, String> {
        let mut count = 0usize;
        let mut total = 0u64;
        copy_dir_bounded(src_dir, &staging_dir, 0, &mut count, &mut total)?;
        let (manifest, content_hash) = validate_plugin_dir(&staging_dir)?;
        let overrides_builtin = builtin_ids.contains(&manifest.id);
        Ok(StagedPlugin {
            manifest,
            staging_dir: staging_dir.clone(),
            content_hash,
            overrides_builtin,
        })
    })();

    if res.is_err() {
        let _ = std::fs::remove_dir_all(&staging_dir);
    }
    res
}

pub fn commit_staged_plugin(staged: StagedPlugin, data_dir: &Path) -> Result<PathBuf, String> {
    let actual = adapter_content_hash(&staged.staging_dir)?;
    if !actual.eq_ignore_ascii_case(&staged.content_hash) {
        let _ = std::fs::remove_dir_all(&staged.staging_dir);
        return Err("暂存插件内容哈希在确认期间发生变更，已中止安装".into());
    }
    let final_dir = plugins_root_dir(data_dir).join(&staged.manifest.id);
    if final_dir.exists() {
        std::fs::remove_dir_all(&final_dir).map_err(|e| format!("清理旧版插件目录失败: {e}"))?;
    }
    std::fs::rename(&staged.staging_dir, &final_dir)
        .map_err(|e| format!("落盘插件目录失败: {e}"))?;
    Ok(final_dir)
}

pub fn discard_staged_plugin(staged: &StagedPlugin) {
    let _ = std::fs::remove_dir_all(&staged.staging_dir);
}

pub fn remove_external_plugin_dir(data_dir: &Path, plugin_id: &str) -> Result<bool, String> {
    if !is_valid_plugin_id(plugin_id) {
        return Err(format!("非法插件 ID: {plugin_id}"));
    }
    let dir = plugins_root_dir(data_dir).join(plugin_id);
    if !dir.exists() {
        return Ok(false);
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("删除插件目录失败: {e}"))?;
    Ok(true)
}

pub fn scan_external_plugins_v2(data_dir: &Path) -> Vec<(PathBuf, PluginManifest, String)> {
    let root = plugins_root_dir(data_dir);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| !n.starts_with('.'))
                && std::fs::symlink_metadata(p)
                    .map(|m| m.is_dir() && !m.file_type().is_symlink())
                    .unwrap_or(false)
        })
        .collect();
    dirs.sort();

    let mut out = Vec::new();
    for dir in dirs {
        let Some(dir_name) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Ok((manifest, hash)) = validate_plugin_dir(&dir) {
            if manifest.id == dir_name {
                out.push((dir, manifest, hash));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn make_zip(path: &Path, entries: Vec<(&str, &[u8], Option<u32>)>) {
        let file = std::fs::File::create(path).unwrap();
        let mut zw = zip::ZipWriter::new(file);
        for (name, data, mode) in entries {
            let mut opts = SimpleFileOptions::default();
            if let Some(m) = mode {
                opts = opts.unix_permissions(m);
            }
            zw.start_file(name, opts).unwrap();
            zw.write_all(data).unwrap();
        }
        zw.finish().unwrap();
    }

    #[test]
    fn rejects_zip_slip_and_symlinks() {
        let root = std::env::temp_dir().join(format!("qwitch_zip_sec_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let builtins = HashSet::new();

        let slip_zip = root.join("slip.zip");
        make_zip(&slip_zip, vec![("../escape.txt", b"evil", None)]);
        let err = stage_zip_plugin(&slip_zip, &root, &builtins).unwrap_err();
        assert!(err.contains("非法") || err.contains("穿越"));

        let sym_zip = root.join("sym.zip");
        {
            let file = std::fs::File::create(&sym_zip).unwrap();
            let mut zw = zip::ZipWriter::new(file);
            zw.add_symlink("manifest.json", "/etc/passwd", SimpleFileOptions::default())
                .unwrap();
            zw.finish().unwrap();
        }
        let err2 = stage_zip_plugin(&sym_zip, &root, &builtins).unwrap_err();
        assert!(err2.contains("符号链接"), "actual err2: {err2}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unwraps_single_outer_folder_and_commits_cleanly() {
        let root = std::env::temp_dir().join(format!("qwitch_zip_ok_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let builtins = HashSet::from(["trae_cn".to_string()]);

        let manifest = serde_json::json!({
            "schemaVersion": 2,
            "id": "qoder_custom",
            "name": "Qoder Custom",
            "version": "1.0.0",
            "kind": "ingress",
            "transport": "declarative",
            "ingressProfile": {
                "wireProtocols": ["openai_chat"]
            }
        })
        .to_string();

        let zip_path = root.join("qoder.zip");
        make_zip(
            &zip_path,
            vec![("qoder-v1/manifest.json", manifest.as_bytes(), None)],
        );

        let staged = stage_zip_plugin(&zip_path, &root, &builtins).unwrap();
        assert_eq!(staged.manifest.id, "qoder_custom");
        assert!(!staged.overrides_builtin);

        let final_dir = commit_staged_plugin(staged, &root).unwrap();
        assert!(final_dir.join("manifest.json").is_file());
        let scanned = scan_external_plugins_v2(&root);
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].1.id, "qoder_custom");

        assert!(remove_external_plugin_dir(&root, "qoder_custom").unwrap());
        assert!(scan_external_plugins_v2(&root).is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stage_json_allows_declarative_and_rejects_subprocess() {
        let root = std::env::temp_dir().join(format!("qwitch_json_stage_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let builtins = HashSet::new();

        let decl_path = root.join("decl.json");
        std::fs::write(
            &decl_path,
            serde_json::json!({
                "schemaVersion": 2,
                "id": "my_agent",
                "name": "My Agent",
                "version": "0.1.0",
                "kind": "ingress",
                "transport": "declarative"
            })
            .to_string(),
        )
        .unwrap();
        let staged = stage_json_plugin(&decl_path, &root, &builtins).unwrap();
        assert_eq!(staged.manifest.id, "my_agent");
        discard_staged_plugin(&staged);

        let sub_path = root.join("sub.json");
        std::fs::write(
            &sub_path,
            serde_json::json!({
                "schemaVersion": 2,
                "id": "my_sub",
                "name": "My Sub",
                "version": "0.1.0",
                "kind": "egress",
                "transport": "subprocess",
                "entry": { "command": "node", "args": ["plugin.js"] }
            })
            .to_string(),
        )
        .unwrap();
        assert!(stage_json_plugin(&sub_path, &root, &builtins).is_err());

        let _ = std::fs::remove_dir_all(&root);
    }
}
