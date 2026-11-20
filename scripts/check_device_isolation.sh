#!/usr/bin/env bash
# =============================================================================
# 校验 Trae CN「隔离 profile 是否拥有独立 device_id」
# -----------------------------------------------------------------------------
# 原理：Trae CN 的 device_id 明文写在 storage.json 的 key 名里：
#         "iCubeAuthInfo://icube-dc:<deviceId>": "..."
#       主目录 与 隔离 profile 各读一次，比较是否不同即可。
#
# 用法：
#   ./check_device_isolation.sh "<profileDir>"
#   例：./check_device_isolation.sh "/tmp/qwitch-prof-b"
#       （profileDir 就是你传给 --user-data-dir 的那个目录）
#
# 退出码：0=PASS(设备不同) 1=待定/FAIL(相同或未登录) 2=用法错误
# =============================================================================
set -uo pipefail

PROFILE="${1:-}"
if [ -z "$PROFILE" ]; then
  echo "用法: $0 <profileDir>   （例：$0 /tmp/qwitch-prof-b）" >&2
  exit 2
fi

MAIN_STORAGE="$HOME/Library/Application Support/Trae CN/User/globalStorage/storage.json"
PROFILE_STORAGE="$PROFILE/User/globalStorage/storage.json"

extract_dev() { # $1=storage.json 路径
  local f="$1"
  [ -f "$f" ] || { echo ""; return; }
  # 从 key 名里抽取 icube-dc: 后的 deviceId（只看 key，无需解密值）
  grep -o 'iCubeAuthInfo://icube-dc:[^"]*' "$f" 2>/dev/null \
    | head -n 1 | sed 's#.*icube-dc:##'
}

MAIN_DEV="$(extract_dev "$MAIN_STORAGE")"
PROF_DEV="$(extract_dev "$PROFILE_STORAGE")"

echo "主目录 storage:   $MAIN_STORAGE"
echo "  device_id = ${MAIN_DEV:-<未找到/未登录>}"
echo "隔离 storage:     $PROFILE_STORAGE"
echo "  device_id = ${PROF_DEV:-<未找到>}"
echo "------------------------------------------------------------"

if [ -z "$MAIN_DEV" ]; then
  echo "结论: ❌ FAIL —— 主目录未找到有效 device_id（请确认在已登录 Trae CN 的 macOS 宿主上运行）。" >&2
  exit 1
fi

if [ -z "$PROF_DEV" ]; then
  echo "结论: ⏳ 待定 —— 隔离 profile 还没有 storage.json 设备键。"
  echo "      请先在隔离实例里【登录一次】目标账号，再重新运行本脚本。"
  exit 1
fi

if [ "$MAIN_DEV" = "$PROF_DEV" ]; then
  echo "结论: ❌ FAIL —— 两处 device_id 相同。"
  echo "      说明该版本 Trae CN 不认 --user-data-dir 做设备隔离，"
  echo "      单机造不出多设备 → 需要改走【多真机 / VM】导入方案（方案 C）。"
  exit 1
else
  echo "结论: ✅ PASS —— 两处 device_id 不同，单机多 profile 独立设备成立（方案 A 可落地）。"
  exit 0
fi
