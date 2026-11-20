#!/usr/bin/env bash
# =============================================================================
# 导出 Trae CN 外部 profile（供 Qwitch「设备 profile」导入采集设备身份）
# -----------------------------------------------------------------------------
# 「导出」= 把某台设备/用户上已登录 Trae CN 的用户数据里，Qwitch 导入所需的
#           最小集合拷到一个目录：
#             <dest>/User/globalStorage/storage.json      (# 认证 + device_id)
#             <dest>/User/globalStorage/state.vscdb        (# 可选，认证缓存)
#             <dest>/aha/TinyStorage                        (# 设备注册信息)
#           Qwitch 导入时把「设备 profile 目录」指向 <dest> 即可。
#
# 用法：
#   # 1) 导出当前用户已登录的 Trae CN（常用于在“第二台机器/VM”上执行后再拷走）
#   ./export_trae_profile.sh --current --dest ~/qwitch-profiles/trae-B
#
#   # 2) 从指定 Trae CN 数据根导出（例如从第二台机器 scp 过来的原始目录）
#   ./export_trae_profile.sh --src "/path/to/Trae CN" --dest ~/qwitch-profiles/trae-B
#
# 说明：在【第二台真机 / 虚拟机 / 第二个 macOS 用户】里登录目标测试账号后运行，
#       导出目录再拷到主机，导入 Qwitch，即可获得“与主机不同”的设备身份。
# =============================================================================
set -uo pipefail
umask 077

SRC=""; DEST=""; CURRENT=0
while [ $# -gt 0 ]; do
  case "$1" in
    --current) CURRENT=1; shift;;
    --src) SRC="$2"; shift 2;;
    --dest) DEST="$2"; shift 2;;
    -h|--help) sed -n '2,30p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0;;
    *) echo "未知参数: $1" >&2; exit 2;;
  esac
done

if [ "$CURRENT" = "1" ]; then SRC="$HOME/Library/Application Support/Trae CN"; fi
[ -n "$SRC" ] || { echo "缺少 --src 或 --current" >&2; exit 2; }
[ -n "$DEST" ] || { echo "缺少 --dest" >&2; exit 2; }

SRC_STORAGE="$SRC/User/globalStorage/storage.json"
if [ ! -f "$SRC_STORAGE" ]; then
  echo "❌ 源目录无 storage.json：$SRC_STORAGE" >&2
  echo "   请确认 --src 指向 Trae CN 的 user-data 根（其下有 User/globalStorage/storage.json）" >&2
  exit 1
fi

echo "== 导出 =="
echo "源: $SRC"
echo "目标: $DEST"
mkdir -p "$DEST/User/globalStorage" "$DEST/aha"
chmod 700 "$DEST" "$DEST/User" "$DEST/User/globalStorage" "$DEST/aha" 2>/dev/null || true
cp -f "$SRC_STORAGE" "$DEST/User/globalStorage/storage.json"
chmod 600 "$DEST/User/globalStorage/storage.json" 2>/dev/null || true
for f in state.vscdb state.vscdb-wal state.vscdb-shm; do
  if [ -f "$SRC/User/globalStorage/$f" ]; then
    cp -f "$SRC/User/globalStorage/$f" "$DEST/User/globalStorage/$f"
    chmod 600 "$DEST/User/globalStorage/$f" 2>/dev/null || true
  fi
done
if [ -f "$SRC/aha/TinyStorage" ]; then
  cp -f "$SRC/aha/TinyStorage" "$DEST/aha/TinyStorage"
  chmod 600 "$DEST/aha/TinyStorage" 2>/dev/null || true
fi

extract_dev() { grep -o 'iCubeAuthInfo://icube-dc:[^"]*' "$1" 2>/dev/null | head -1 | sed 's#.*icube-dc:##'; }
DEST_DEV="$(extract_dev "$DEST/User/globalStorage/storage.json")"
MAIN_DEV="$(extract_dev "$HOME/Library/Application Support/Trae CN/User/globalStorage/storage.json" 2>/dev/null)"

echo
echo "== 校验 =="
echo "导出目录 device_id = ${DEST_DEV:-<未找到>}"
echo "本机主目录 device_id = ${MAIN_DEV:-<未找到>}"
echo "导出结构："; find "$DEST" -maxdepth 3 -type f 2>/dev/null

echo
if [ -z "$DEST_DEV" ]; then
  echo "⚠️ 导出目录未发现 device_id（该 profile 可能尚未完成登录）" >&2
  exit 1
elif [ "$CURRENT" != "1" ] && [ -n "$MAIN_DEV" ] && [ "$DEST_DEV" = "$MAIN_DEV" ]; then
  echo "⚠️ 导出设备与主机相同 —— 这仍是同一台设备，不能用于多账号签到。"
  echo "   请在【第二台机器/VM/第二个 macOS 用户】里登录目标账号后再导出。"
  exit 1
elif [ "$CURRENT" = "1" ]; then
  echo "✅ 本机导出完成：请将 $DEST 拷贝到另一台主机再导入 Qwitch（仅作跨机迁移）。"
else
  echo "✅ 导出成功，且设备与主机不同（可用于多设备签到）。"
  echo "   Qwitch 导入时把「设备 profile 目录」填为：$DEST"
fi
