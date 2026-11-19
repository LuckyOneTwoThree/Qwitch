#!/usr/bin/env bash
# =============================================================================
# Trae CN 设备身份探针 (read-only 优先 / claim 需显式确认)
# -----------------------------------------------------------------------------
# 目的：用一个/多个账号 Token，改变 x-device-id 等设备头，观察服务端返回码，
#       判定「设备日限到底是不是按 x-device-id 绑、以及 device_id 能否被断言」。
#
# 安全设计：
#   * status 模式：只查签到状态，不消耗每日额度（非破坏性）。
#   * claim  模式：会真实消耗账号当日签到额度，必须 CONFIRM_CLAIM=YES 才执行，
#                 逐账号串行 + 抖动，命中 403/429 立即中止。
#   * 不打印完整 Token（只显示前 8 位 + 长度）。
#
# 用法：
#   ./trae_probe.sh status --token "$TOKEN" [--alt-device ID] [--dry-run]
#   CONFIRM_CLAIM=YES ./trae_probe.sh claim --accounts accts.tsv [--alt-device ID]
#
#   accts.tsv 每行: label <TAB> token <TAB> [device_id]   （第三列可选，缺省用本机真实 ID）
#
# 环境变量：
#   TRAE_API   默认 https://api.trae.cn
#   TRAE_APPVER 默认 3.4.22
#   CONFIRM_CLAIM   claim 模式必须 =YES
# =============================================================================
set -uo pipefail

API="${TRAE_API:-https://api.trae.cn}"
APPVER="${TRAE_APPVER:-3.4.22}"
PROBE_DIR="${PROBE_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/probe-out}"
mkdir -p "$PROBE_DIR"
TS="$(date +%Y%m%d-%H%M%S)"
LOG="$PROBE_DIR/trae-probe-$TS.tsv"

stat_path="/trae/api/v2/ug/checkin_credits/status"
claim_path="/trae/api/v2/ug/checkin_credits/claim"

# ---------- 本机设备信息（与 adapter/trae.rs 的 TraeDeviceHeaders 对齐）----------
detect_real_device_id() {
  python3 - "$HOME/Library/Application Support/Trae CN/User/globalStorage/storage.json" <<'PY' 2>/dev/null
import json,sys
try:
    d=json.load(open(sys.argv[1]))
    for k in d:
        if k.startswith("iCubeAuthInfo://icube-dc:"):
            print(k.split("iCubeAuthInfo://icube-dc:",1)[1]); break
except Exception:
    pass
PY
}
DEVICE_BRAND="$(sysctl -n hw.model 2>/dev/null || echo '')"
OS_VERSION="macOS $(sw_vers -productVersion 2>/dev/null || echo '')"
case "$(uname -s)" in Darwin) DEVICE_TYPE="mac";; Linux) DEVICE_TYPE="linux";; *) DEVICE_TYPE="win";; esac

mask_token() { local t="$1"; local n=${#t}; if [ "$n" -le 12 ]; then printf '***'; else printf '%s…(%d)' "${t:0:8}" "$n"; fi; }

# ---------- 发送请求：结果写入 RESP_CODE / RESP_CODE_BIZ / RESP_JSON ----------
do_request() {
  local path="$1" token="$2" device_id="$3"; shift 3
  local extra=("$@")
  local args=(
    -s -m 20 -o "$PROBE_DIR/.resp.$$" -w '%{http_code}'
    -X POST "$API$path"
    -H "Accept: application/json"
    -H "Content-Type: application/json"
    -H "Authorization: Cloud-IDE-JWT $token"
    -H "X-User-Region: CN"
    -H "x-device-type: $DEVICE_TYPE"
    -H "x-app-version: $APPVER"
  )
  [ -n "$DEVICE_BRAND" ] && args+=(-H "x-device-brand: $DEVICE_BRAND")
  [ -n "$OS_VERSION" ] && args+=(-H "x-os-version: $OS_VERSION")
  # device_id 为空 => 故意不带该头，验证 9004
  if [ -n "$device_id" ]; then args+=(-H "x-device-id: $device_id"); fi
  args+=(-d '{"req_source":1}')
  if [ "${#extra[@]}" -gt 0 ]; then args+=("${extra[@]}"); fi

  if [ "${DRY_RUN:-0}" = "1" ]; then
    # 打印将发送的命令（device 明示，token 脱敏）
    echo "+ curl ${args[*]//$token/$(mask_token "$token")}" >&2
    RESP_CODE="dry"; RESP_CODE_BIZ="dry"; RESP_JSON='{"dry_run":true}'; return 0
  fi
  RESP_CODE="$(curl "${args[@]}")" || RESP_CODE="000"
  RESP_JSON="$(cat "$PROBE_DIR/.resp.$$" 2>/dev/null || echo '')"
  rm -f "$PROBE_DIR/.resp.$$"
  RESP_CODE_BIZ="$(printf '%s' "$RESP_JSON" | jq -r '.code // empty' 2>/dev/null || echo '')"
}

# 单次探测打印 + 记录
probe_one() {
  local label="$1" path="$2" token="$3" device_id="$4"
  do_request "$path" "$token" "$device_id"
  local checked_in today_checked
  checked_in="$(printf '%s' "$RESP_JSON" | jq -r '.data.checked_in // .data.did_checked_in // empty' 2>/dev/null || echo '')"
  today_checked="$(printf '%s' "$RESP_JSON" | jq -r '.data.today_checked_in // empty' 2>/dev/null || echo '')"
  local dev_show="${device_id:-<缺省>}"
  printf '%-22s http=%-3s code=%-5s checked_in=%-5s today=%s\n' \
    "$label" "$RESP_CODE" "${RESP_CODE_BIZ:-?}" "${checked_in:-?}" "${today_checked:-?}"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$(date +%H:%M:%S)" "$path" "$label" "$dev_show" "$RESP_CODE" "${RESP_CODE_BIZ:-}" "${checked_in:-}" "$RESP_JSON" >> "$LOG"
}

# =============================================================================
cmd_status() {
  local token="$1" alt="$2"
  [ -n "$token" ] || { echo "status 模式需要 --token" >&2; return 2; }
  local real; real="$(detect_real_device_id)"
  local rnd; rnd="$(uuidgen | tr -d '-' | tr 'A-Z' 'a-z')"
  echo "== 探针：status（非破坏性）token=$(mask_token "$token") =="
  echo "   本机真实 device_id = ${real:-<未探测到>}"
  probe_one "real(本机真实)"   "$stat_path" "$token" "$real"
  [ -n "$alt" ] && probe_one "alt(第二台真实)" "$stat_path" "$token" "$alt"
  probe_one "random(随机新ID)" "$stat_path" "$token" "$rnd"
  probe_one "missing(缺省头)"  "$stat_path" "$token" ""
  echo "== 结果已写入 $LOG =="
}

# =============================================================================
cmd_claim() {
  local accts="$1" alt="$2"
  if [ "${CONFIRM_CLAIM:-}" != "YES" ]; then
    echo "!! claim 模式会真实消耗账号当日签到额度。确认请设 CONFIRM_CLAIM=YES" >&2; return 3
  fi
  [ -f "$accts" ] || { echo "缺少 accounts 文件: $accts（每行 label<TAB>token[<TAB>device_id]）" >&2; return 2; }
  local real; real="$(detect_real_device_id)"
  echo "== 探针：claim（破坏性，逐账号串行 + 抖动）=="

  # 关键实验：每个账号用「它自己的 device_id」各领一次；再对同一账号跨设备重复领，观察 9095
  local i=0
  while IFS=$'\t' read -r label token devid; do
    [ -z "${token:-}" ] && continue
    [ -z "${devid:-}" ] && devid="$real"
    i=$((i+1))
    echo "---- [$i] $label  device=${devid:-<缺省>} ----"
    probe_one "claim:$label/dev" "$claim_path" "$token" "$devid"
    # 跨设备验证：同一 token 换 alt（若有）再打一次，预期命中 9095 或风控
    if [ -n "$alt" ] && [ "$alt" != "$devid" ]; then
      sleep 2
      probe_one "claim:$label/alt" "$claim_path" "$token" "$alt"
    fi
    # 风控中止
    case "$RESP_CODE" in 403|429) echo "!! 命中 $RESP_CODE，按防风控立即中止整批" >&2; break;; esac
    if [ "$RESP_CODE_BIZ" = "9095" ]; then echo "   (9095: 该设备今日已签 —— 若这是第二个账号，说明设备日限生效)"; fi
    # 串行抖动 1.5~3.5s（与产品一致）
    local jit=$((1500 + RANDOM % 2000))
    sleep "$(awk -v m="$jit" 'BEGIN{printf "%.2f", m/1000}')"
  done < "$accts"
  echo "== 结果已写入 $LOG =="
}

usage() { sed -n '2,30p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

# ---------- 解析 ----------
MODE="${1:-}"; shift || true
TOKEN=""; ALT=""; ACCTS=""
while [ $# -gt 0 ]; do
  case "$1" in
    --token) TOKEN="$2"; shift 2;;
    --alt-device) ALT="$2"; shift 2;;
    --accounts) ACCTS="$2"; shift 2;;
    --dry-run) DRY_RUN=1; shift;;
    -h|--help) usage; exit 0;;
    *) echo "未知参数: $1" >&2; usage; exit 2;;
  esac
done

case "$MODE" in
  status) cmd_status "$TOKEN" "$ALT";;
  claim)  cmd_claim "$ACCTS" "$ALT";;
  *) usage; exit 2;;
esac
