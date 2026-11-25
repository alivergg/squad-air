#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

TIERS_CSV="1,2,3"
FEATURES_CSV=""
OPT_ADB=0
OPT_REQUIRE_ADB=0
OPT_INSTALL_ADB=0
OPT_BUILD_APK=0
OPT_RELEASE_GATE=0
OPT_FULL=0

PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0

usage() {
  cat <<'EOF'
Usage: scripts/verify_e2e_requirements.sh [OPTIONS]

Opaque-box requirement-driven E2E verification harness for SquadAir (F1–F13 / R1–R7).

Options:
  --tier <1|2|3|4>     Tiers to execute (comma-separated or repeatable; default: 1,2,3)
  --pre-merge          Run non-destructive Tiers 1–3
  --full               Run Tiers 1–4 (includes APK build & ADB checks when attached)
  --feature <F1..F13>  Filter checks by feature ID (comma-separated or repeatable)
  --adb                Run non-destructive ADB checks if Samsung S10 / arm64 device attached
  --require-adb        Fail if no authorized arm64 ADB device is attached
  --install-adb        Run `adb install -r dist/SquadAir-android-arm64.apk` before ADB checks
  --build-apk          Run `./scripts/build_android_apk.sh` in Tier 4
  --release-gate       Verify `main` branch, tracked `dist/SquadAir-android-arm64.apk`, and `assets/img/`
  --help               Show this help message
EOF
}

append_csv() {
  local cur="$1"
  local add="$2"
  if [[ -z "$cur" ]]; then
    echo "$add"
  else
    echo "${cur},${add}"
  fi
}

validate_tiers_csv() {
  local raw="$1"
  local tok
  local -a tokens=()
  if [[ -z "$raw" || "$raw" == *, ]]; then
    echo "Invalid --tier value: '$raw' (expected 1..4)" >&2
    exit 2
  fi
  set -f
  IFS=',' read -ra tokens <<< "$raw"
  set +f
  if [[ ${#tokens[@]} -eq 0 ]]; then
    echo "Invalid --tier value: '$raw' (expected 1..4)" >&2
    exit 2
  fi
  for tok in "${tokens[@]}"; do
    if [[ ! "$tok" =~ ^[1-4]$ ]]; then
      echo "Invalid --tier token: '$tok' (expected 1..4)" >&2
      exit 2
    fi
  done
}

validate_features_csv() {
  local raw="$1"
  local tok
  local -a tokens=()
  if [[ -z "$raw" || "$raw" == *, ]]; then
    echo "Invalid --feature value: '$raw' (expected F1..F13)" >&2
    exit 2
  fi
  set -f
  IFS=',' read -ra tokens <<< "$raw"
  set +f
  if [[ ${#tokens[@]} -eq 0 ]]; then
    echo "Invalid --feature value: '$raw' (expected F1..F13)" >&2
    exit 2
  fi
  for tok in "${tokens[@]}"; do
    if [[ ! "$tok" =~ ^F([1-9]|1[0-3])$ ]]; then
      echo "Invalid --feature token: '$tok' (expected F1..F13)" >&2
      exit 2
    fi
  done
}

TIERS_EXPLICIT=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --tier)
      [[ $# -ge 2 ]] || { echo "Missing value for --tier" >&2; exit 2; }
      validate_tiers_csv "$2"
      if [[ $TIERS_EXPLICIT -eq 0 ]]; then
        TIERS_CSV="$2"
        TIERS_EXPLICIT=1
      else
        TIERS_CSV="$(append_csv "$TIERS_CSV" "$2")"
      fi
      shift 2
      ;;
    --tier=*)
      val="${1#*=}"
      validate_tiers_csv "$val"
      if [[ $TIERS_EXPLICIT -eq 0 ]]; then
        TIERS_CSV="$val"
        TIERS_EXPLICIT=1
      else
        TIERS_CSV="$(append_csv "$TIERS_CSV" "$val")"
      fi
      shift
      ;;
    --pre-merge)
      if [[ $TIERS_EXPLICIT -eq 0 ]]; then
        TIERS_CSV="1,2,3"
      fi
      shift
      ;;
    --full)
      if [[ $TIERS_EXPLICIT -eq 0 ]]; then
        TIERS_CSV="1,2,3,4"
      fi
      OPT_FULL=1
      OPT_BUILD_APK=1
      OPT_ADB=1
      shift
      ;;
    --feature)
      [[ $# -ge 2 ]] || { echo "Missing value for --feature" >&2; exit 2; }
      f_upper="$(printf '%s' "$2" | tr '[:lower:]' '[:upper:]')"
      validate_features_csv "$f_upper"
      FEATURES_CSV="$(append_csv "$FEATURES_CSV" "$f_upper")"
      shift 2
      ;;
    --feature=*)
      f_upper="$(printf '%s' "${1#*=}" | tr '[:lower:]' '[:upper:]')"
      validate_features_csv "$f_upper"
      FEATURES_CSV="$(append_csv "$FEATURES_CSV" "$f_upper")"
      shift
      ;;
    --adb)
      OPT_ADB=1
      shift
      ;;
    --require-adb)
      OPT_ADB=1
      OPT_REQUIRE_ADB=1
      shift
      ;;
    --install-adb)
      OPT_ADB=1
      OPT_INSTALL_ADB=1
      shift
      ;;
    --build-apk)
      OPT_BUILD_APK=1
      shift
      ;;
    --release-gate)
      OPT_RELEASE_GATE=1
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

tier_enabled() {
  local target="$1"
  [[ ",${TIERS_CSV}," == *",${target},"* ]]
}

feature_enabled() {
  local tags="$1"
  if [[ -z "$FEATURES_CSV" ]]; then
    return 0
  fi
  local -a feats=()
  local f
  set -f
  IFS=',' read -ra feats <<< "$FEATURES_CSV"
  set +f
  for f in "${feats[@]}"; do
    if [[ ",${tags}," == *",${f},"* ]]; then
      return 0
    fi
  done
  return 1
}

log_pass() {
  local tier="$1" feat="$2" id="$3" msg="$4"
  echo "[PASS] [Tier ${tier}][${feat}] ${id}: ${msg}"
  PASS_COUNT=$((PASS_COUNT + 1))
}

log_fail() {
  local tier="$1" feat="$2" id="$3" msg="$4"
  echo "[FAIL] [Tier ${tier}][${feat}] ${id}: ${msg}"
  FAIL_COUNT=$((FAIL_COUNT + 1))
}

log_skip() {
  local tier="$1" feat="$2" id="$3" msg="$4"
  echo "[SKIP] [Tier ${tier}][${feat}] ${id}: ${msg}"
  SKIP_COUNT=$((SKIP_COUNT + 1))
}

PY_HELPERS='
import pathlib, re, sys

def skip_non_code(src, i, rust=False):
    n = len(src)
    if i >= n:
        return i
    ch = src[i]
    if ch == "/" and i + 1 < n and src[i + 1] == "/":
        i += 2
        while i < n and src[i] != "\n":
            i += 1
        return i
    if ch == "/" and i + 1 < n and src[i + 1] == "*":
        d = 1
        i += 2
        while i < n and d > 0:
            if src[i:i + 2] == "/*":
                d += 1
                i += 2
            elif src[i:i + 2] == "*/":
                d -= 1
                i += 2
            else:
                i += 1
        return i
    if rust and ch in ("r", "b") and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == "_")):
        m_raw = re.match(r"(?:br|rb|r)(#*)\"", src[i:])
        if m_raw:
            end_tok = "\"" + m_raw.group(1)
            end_idx = src.find(end_tok, i + len(m_raw.group(0)))
            if end_idx != -1:
                return end_idx + len(end_tok)
    if not rust and src[i:i + 3] == "\"\"\"":
        end_idx = src.find("\"\"\"", i + 3)
        if end_idx != -1:
            return end_idx + 3
    if ch == "\"":
        i += 1
        while i < n:
            if src[i] == "\\":
                i += 2
                continue
            if src[i] == "\"":
                return i + 1
            i += 1
        return i
    if ch == "\x27":
        if i + 1 < n and src[i + 1] == "\\":
            if i + 2 < n and src[i + 2] != "\n":
                k = i + 3
                while k < n and src[k] not in ("\x27", "\n"):
                    k += 1
                if k < n and src[k] == "\x27":
                    return k + 1
        elif i + 2 < n and src[i + 1] != "\n" and src[i + 2] == "\x27":
            return i + 3
    return i

def strip_comments(src, rust=False, mask_strings=False, mask_chars=False):
    out = []
    i = 0
    n = len(src)
    while i < n:
        ch = src[i]
        if ch == "/" and i + 1 < n and src[i + 1] == "/":
            i += 2
            while i < n and src[i] != "\n":
                out.append(" ")
                i += 1
            continue
        if ch == "/" and i + 1 < n and src[i + 1] == "*":
            depth = 1
            out.append("  ")
            i += 2
            while i < n and depth > 0:
                if src[i] == "/" and i + 1 < n and src[i + 1] == "*":
                    depth += 1
                    out.append("  ")
                    i += 2
                elif src[i] == "*" and i + 1 < n and src[i + 1] == "/":
                    depth -= 1
                    out.append("  ")
                    i += 2
                elif src[i] == "\n":
                    out.append("\n")
                    i += 1
                else:
                    out.append(" ")
                    i += 1
            continue
        if rust and ch in ("r", "b") and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == "_")):
            m_raw = re.match(r"(?:br|rb|r)(#*)\"", src[i:])
            if m_raw:
                hashes = m_raw.group(1)
                prefix_len = len(m_raw.group(0))
                end_tok = "\"" + hashes
                end_idx = src.find(end_tok, i + prefix_len)
                if end_idx != -1:
                    seg = src[i:end_idx + len(end_tok)]
                    if mask_strings:
                        out.append("".join("\n" if c == "\n" else " " for c in seg))
                    else:
                        out.append(seg)
                    i = end_idx + len(end_tok)
                    continue
        if not rust and src[i:i + 3] == "\"\"\"":
            end_idx = src.find("\"\"\"", i + 3)
            if end_idx != -1:
                seg = src[i:end_idx + 3]
                if mask_strings:
                    out.append("".join("\n" if c == "\n" else " " for c in seg))
                else:
                    out.append(seg)
                i = end_idx + 3
                continue
        if ch == "\"":
            j = i + 1
            while j < n:
                if src[j] == "\\":
                    j += 2
                    continue
                if src[j] == "\"":
                    j += 1
                    break
                j += 1
            seg = src[i:j]
            if mask_strings:
                out.append("\"" + "".join("\n" if c == "\n" else " " for c in seg[1:-1]) + ("\"" if len(seg) >= 2 and seg[-1] == "\"" else ""))
            else:
                out.append(seg)
            i = j
            continue
        if ch == "\x27":
            j = None
            if i + 1 < n and src[i + 1] == "\\":
                if i + 2 < n and src[i + 2] != "\n":
                    k = i + 3
                    while k < n and src[k] not in ("\x27", "\n"):
                        k += 1
                    if k < n and src[k] == "\x27":
                        j = k + 1
            elif i + 2 < n and src[i + 1] != "\n" and src[i + 2] == "\x27":
                j = i + 3
            if j is not None:
                seg = src[i:j]
                if mask_chars:
                    out.append("\x27" + (" " * (len(seg) - 2)) + "\x27")
                else:
                    out.append(seg)
                i = j
                continue
        out.append(ch)
        i += 1
    return "".join(out)

def extract_delim_expr(src, open_idx, open_ch, close_ch, rust=False):
    depth = 0
    i = open_idx
    n = len(src)
    while i < n:
        ni = skip_non_code(src, i, rust=rust)
        if ni != i:
            i = ni
            continue
        ch = src[i]
        if ch == open_ch:
            depth += 1
        elif ch == close_ch:
            depth -= 1
            if depth == 0:
                return src[open_idx + 1:i], i
        i += 1
    return None, -1

def _extract_kotlin_expr_body(src, start_idx):
    n = len(src)
    i = start_idx
    d_paren = d_brack = d_brace = 0
    seen_token = False
    last_non_ws = ""
    while i < n:
        ni = skip_non_code(src, i, rust=False)
        if ni != i:
            seen_token = True
            last_non_ws = src[ni - 1]
            i = ni
            continue
        ch = src[i]
        if ch in (" ", "\t", "\r"):
            i += 1
            continue
        if ch == "\n":
            if d_paren == 0 and d_brack == 0 and d_brace == 0 and seen_token:
                k = i + 1
                while k < n and src[k] in (" ", "\t", "\r", "\n"):
                    k += 1
                nl_end = src.find("\n", k)
                rest_line = src[k:nl_end if nl_end != -1 else n].lstrip()
                cont_line = bool(re.match(r"^(?:else\b|\.|&&|\|\||\?:|\?\.)|^[+\-*/%<>=!]+(?!=)", rest_line))
                cont_prev = last_non_ws in (".", "+", "-", "*", "/", "%", "&", "|", "?", ":", ",", "=")
                if not cont_line and not cont_prev:
                    return src[start_idx:i].strip()
            i += 1
            continue
        seen_token = True
        if ch == "(":
            d_paren += 1
        elif ch == ")":
            if d_paren == 0:
                return src[start_idx:i].strip()
            d_paren -= 1
        elif ch == "[":
            d_brack += 1
        elif ch == "]":
            if d_brack == 0:
                return src[start_idx:i].strip()
            d_brack -= 1
        elif ch == "{":
            d_brace += 1
        elif ch == "}":
            if d_brace == 0:
                return src[start_idx:i].strip()
            d_brace -= 1
        elif ch == ";" and d_paren == 0 and d_brack == 0 and d_brace == 0:
            return src[start_idx:i].strip()
        last_non_ws = ch
        i += 1
    return src[start_idx:].strip()

def extract_fn_body(src, fn_pattern, rust=True):
    n = len(src)
    for m in re.finditer(fn_pattern, src):
        paren_idx = src.find("(", m.start())
        if paren_idx == -1:
            continue
        _, close_paren = extract_delim_expr(src, paren_idx, "(", ")", rust=rust)
        if close_paren == -1:
            continue
        i = close_paren + 1
        d_paren = d_brack = 0
        while i < n:
            ni = skip_non_code(src, i, rust=rust)
            if ni != i:
                i = ni
                continue
            ch = src[i]
            if ch == "(":
                d_paren += 1
            elif ch == ")":
                if d_paren > 0:
                    d_paren -= 1
            elif ch == "[":
                d_brack += 1
            elif ch == "]":
                if d_brack > 0:
                    d_brack -= 1
            elif d_paren == 0 and d_brack == 0:
                if ch == ";":
                    break
                if not rust and re.match(r"fun\b", src[i:]) and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == "_")):
                    break
                if not rust and ch == "=" and not (i + 1 < n and src[i + 1] in ("=", ">")):
                    return _extract_kotlin_expr_body(src, i + 1)
                if ch == "{":
                    body, _ = extract_delim_expr(src, i, "{", "}", rust=rust)
                    return body
            i += 1
    return None
'

run_py_check() {
  local tier="$1" feat="$2" id="$3" script="$4"
  feature_enabled "$feat" || return 0
  local out
  if out="$(python3 -c "${PY_HELPERS}"$'\n'"${script}" 2>&1)"; then
    log_pass "$tier" "$feat" "$id" "${out:-ok}"
  else
    log_fail "$tier" "$feat" "$id" "${out:-failed}"
  fi
}

run_cmd_check() {
  local tier="$1" feat="$2" id="$3" desc="$4"
  shift 4
  feature_enabled "$feat" || return 0
  local tmp
  tmp="$(mktemp)"
  if "$@" >"$tmp" 2>&1; then
    rm -f "$tmp"
    log_pass "$tier" "$feat" "$id" "$desc"
  else
    local tail_out
    tail_out="$(tail -n 20 "$tmp" | tr '\n' ' ' | sed 's/  */ /g')"
    rm -f "$tmp"
    log_fail "$tier" "$feat" "$id" "$desc -> ${tail_out}"
  fi
}

find_java_home() {
  if [[ -n "${JAVA_HOME:-}" && -x "${JAVA_HOME}/bin/java" ]]; then
    echo "$JAVA_HOME"
    return 0
  fi
  if [[ -x "/usr/libexec/java_home" ]]; then
    local jh17
    jh17="$(/usr/libexec/java_home -v 17 2>/dev/null || true)"
    if [[ -n "$jh17" && -x "${jh17}/bin/java" ]]; then
      echo "$jh17"
      return 0
    fi
  fi
  local candidates=(
    "/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home"
    "/usr/local/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home"
    "/Library/Java/JavaVirtualMachines/openjdk-17.jdk/Contents/Home"
    "/usr/lib/jvm/java-17-openjdk-amd64"
    "/usr/lib/jvm/java-17-openjdk"
  )
  local c
  for c in "${candidates[@]}"; do
    if [[ -x "${c}/bin/java" ]]; then
      echo "$c"
      return 0
    fi
  done
  if command -v java >/dev/null 2>&1; then
    local java_bin java_home_guess
    java_bin="$(command -v java)"
    java_home_guess="$(cd "$(dirname "$java_bin")/.." 2>/dev/null && pwd || true)"
    if [[ -n "$java_home_guess" && -x "${java_home_guess}/bin/java" ]]; then
      echo "$java_home_guess"
      return 0
    fi
  fi
  return 1
}

find_build_tools() {
  local sdk_candidates=()
  [[ -n "${ANDROID_HOME:-}" ]] && sdk_candidates+=("$ANDROID_HOME")
  [[ -n "${ANDROID_SDK_ROOT:-}" ]] && sdk_candidates+=("$ANDROID_SDK_ROOT")
  sdk_candidates+=("$HOME/Library/Android/sdk" "$HOME/Android/Sdk")
  local sdk latest
  for sdk in "${sdk_candidates[@]}"; do
    if [[ -d "${sdk}/build-tools" ]]; then
      latest="$(ls -d "${sdk}"/build-tools/* 2>/dev/null | sort -V | tail -n1 || true)"
      if [[ -n "$latest" && -x "${latest}/aapt2" ]]; then
        echo "$latest"
        return 0
      fi
    fi
  done
  return 1
}

find_android_ndk() {
  if [[ -n "${ANDROID_NDK_HOME:-}" && -d "$ANDROID_NDK_HOME" ]]; then
    echo "$ANDROID_NDK_HOME"
    return 0
  fi
  local candidate latest=""
  for candidate in \
    "${ANDROID_HOME:-}/ndk/"* \
    "${ANDROID_SDK_ROOT:-}/ndk/"* \
    "$HOME/Library/Android/sdk/ndk/"* \
    "$HOME/Android/Sdk/ndk/"*; do
    if [[ -d "$candidate" && -f "$candidate/build/cmake/android.toolchain.cmake" ]]; then
      latest="$candidate"
    fi
  done
  if [[ -n "$latest" ]]; then
    echo "$latest"
    return 0
  fi
  return 1
}

# ==============================================================================
# TIER 1 — Static, AST, Contract & Fast Unit Tests
# ==============================================================================
if tier_enabled 1; then
  run_py_check 1 "F1" "T1-F1-1" '
aa_path = pathlib.Path("crates/squad-dsp/src/android_audio.rs")
eng_path = pathlib.Path("crates/squad-dsp/src/engine.rs")
hp_path = pathlib.Path("crates/squad-dsp/src/hotplug.rs")
mob_path = pathlib.Path("crates/squad-core/src/mobile_api.rs")
mesh_path = pathlib.Path("crates/squad-core/src/net/mesh.rs")
for p in (aa_path, eng_path, hp_path, mob_path, mesh_path):
    if not p.exists():
        sys.exit(f"missing {p}")

aa = strip_comments(aa_path.read_text(), rust=True)
eng = strip_comments(eng_path.read_text(), rust=True)
hp = strip_comments(hp_path.read_text(), rust=True)
mob = strip_comments(mob_path.read_text(), rust=True)
mesh = strip_comments(mesh_path.read_text(), rust=True)

for label, pat in [
    ("VoiceCommunication usage", r"VOICE_?COMMUNICATION|VoiceCommunication"),
    ("Speech content type", r"CONTENT_TYPE_SPEECH\b|ContentType::Speech"),
    ("LowLatency performance mode", r"LOW_?LATENCY|LowLatency"),
    ("VoiceCommunication input preset", r"INPUT_?PRESET_VOICE_?COMMUNICATION|InputPreset::VoiceCommunication"),
]:
    if not re.search(pat, aa, re.I):
        sys.exit(f"android_audio.rs missing {label}")

if "@id:" not in aa or "parse_android_device_spec" not in aa:
    sys.exit("android_audio.rs missing @id: device spec parser")
if "@id:" not in hp or "parse_android_device_spec" not in eng:
    sys.exit("engine.rs/hotplug.rs missing Android @id: routing integration")

mesh_cue_hits = len(re.findall(r"SoundCue::IncomingCall\b", mesh))
mob_cue_hits = len(re.findall(r"SoundCue::IncomingCall\b", mob))
if mesh_cue_hits != 1 or mob_cue_hits != 0:
    sys.exit(f"IncomingCall cue must fire once in mesh.rs and 0 times in mobile_api.rs (mesh={mesh_cue_hits}, mobile_api={mob_cue_hits})")
print("Android VoIP audio attributes, @id: routing & IncomingCall cue deduplication verified")
'

  run_py_check 1 "F4" "T1-F4-1" '
service_files = sorted(pathlib.Path("android/app/src/main/java/app/squadair/mobile/service").glob("*.kt"))
ctrl_path = pathlib.Path("android/app/src/main/java/app/squadair/mobile/service/AudioRouteController.kt")
if not ctrl_path.exists():
    sys.exit("AudioRouteController.kt missing")
ctrl_src = strip_comments(ctrl_path.read_text(), rust=False)
service_src = "\n".join(strip_comments(f.read_text(), rust=False) for f in service_files)

# Inspect ALL brace-scoped SDK_INT >= Build.VERSION_CODES.S if-blocks
for m in re.finditer(r"\bif\s*\(", ctrl_src):
    paren_idx = ctrl_src.find("(", m.start())
    cond, close_paren = extract_delim_expr(ctrl_src, paren_idx, "(", ")", rust=False)
    if cond is None:
        continue
    if not re.search(r"Build\.VERSION\.SDK_INT\s*>=\s*Build\.VERSION_CODES\.S\b", cond):
        continue
    pos = close_paren + 1
    while pos < len(ctrl_src) and ctrl_src[pos].isspace():
        pos += 1
    if pos < len(ctrl_src) and ctrl_src[pos] == "{":
        block, _ = extract_delim_expr(ctrl_src, pos, "{", "}", rust=False)
        if block is None:
            sys.exit("unclosed SDK_INT >= S block in AudioRouteController.kt")
    else:
        line_end = ctrl_src.find("\n", pos)
        stmt_end = ctrl_src.find(";", pos)
        ends = [e for e in (line_end, stmt_end) if e != -1]
        end_pos = min(ends) if ends else len(ctrl_src)
        block = ctrl_src[pos:end_pos]
    if "isSpeakerphoneOn" in block:
        sys.exit("deprecated isSpeakerphoneOn still called inside an SDK_INT >= S branch")

if "OnCommunicationDeviceChangedListener" not in ctrl_src:
    sys.exit("missing OnCommunicationDeviceChangedListener on API 31+")
if "\"@id:" not in service_src and "@id:" not in service_src:
    sys.exit("missing @id: AudioDeviceInfo.id routing spec")

deact_body = extract_fn_body(ctrl_src, r"\bfun\s+deactivate\s*\(", rust=False)
if deact_body is None or not re.search(r"\(\s*null\s*,\s*null\s*\)", deact_body):
    sys.exit("deactivate() must clear Rust route overrides via (null, null)")

if not re.search(r"\"bottom\"", service_src, re.I):
    sys.exit("missing bottom built-in mic selection logic in service/*.kt")
if not re.search(r"(TYPE_WIRED_HEADSET|TYPE_USB_HEADSET|TYPE_BLUETOOTH_SCO|TYPE_BLE_HEADSET)", service_src):
    sys.exit("missing wired/Bluetooth headset mic selection logic in service/*.kt")
print("AudioRouteController API 31+, @id:, mic & deactivate contracts verified")
'

  run_py_check 1 "F4" "T1-F4-2" '
core_files = sorted(pathlib.Path("android/app/src/main/java/app/squadair/mobile/core").glob("*.kt"))
service_files = sorted(pathlib.Path("android/app/src/main/java/app/squadair/mobile/service").glob("*.kt"))
repo_path = pathlib.Path("android/app/src/main/java/app/squadair/mobile/core/SquadSessionRepository.kt")
if not repo_path.exists():
    sys.exit("SquadSessionRepository.kt missing")

core_src = "\n".join(strip_comments(f.read_text(), rust=False) for f in core_files)
combined = core_src + "\n" + "\n".join(strip_comments(f.read_text(), rust=False) for f in service_files)
repo_src = strip_comments(repo_path.read_text(), rust=False)

has_speaker_check = bool(re.search(r"==\s*AudioRoute\.SPEAKER\b", combined))
if not has_speaker_check:
    for m_when in re.finditer(r"\bwhen\s*\(", combined):
        paren_idx = combined.find("(", m_when.start())
        _, close_paren = extract_delim_expr(combined, paren_idx, "(", ")", rust=False)
        if close_paren == -1:
            continue
        pos = close_paren + 1
        while pos < len(combined) and combined[pos].isspace():
            pos += 1
        if pos < len(combined) and combined[pos] == "{":
            when_body, _ = extract_delim_expr(combined, pos, "{", "}", rust=False)
            if when_body and re.search(r"\bAudioRoute\.SPEAKER\b", when_body):
                has_speaker_check = True
                break
if not has_speaker_check:
    sys.exit("missing AudioRoute.SPEAKER route-change handling in core/service Kotlin")

has_baseline_save = bool(re.search(r"(setBaselineAec|getBaselineAec|baseline_aec|preSpeakerAec|baselineAec|savedAec)", combined))
has_speaker_override = bool(re.search(r"(isSpeakerOverrideActive|setSpeakerOverrideActive|speaker_aec_override|effectiveAec\s*=\s*true)", combined))
if not (has_baseline_save and has_speaker_override):
    sys.exit("missing speaker AEC override + baseline save/restore semantics in core/service Kotlin")

if "setAecEnabled(" not in repo_src:
    sys.exit("SquadSessionRepository.kt missing controller.setAecEnabled(...) wiring")
print("Speaker AEC auto-enable & baseline save/restore semantics verified")
'

  run_py_check 1 "F5" "T1-F5-1" '
p = pathlib.Path("android/app/src/main/AndroidManifest.xml")
src = re.sub(r"<!--[\s\S]*?-->", "", p.read_text())
missing = [perm for perm in ("USE_FULL_SCREEN_INTENT", "REQUEST_IGNORE_BATTERY_OPTIMIZATIONS") if perm not in src]
if missing:
    sys.exit(f"AndroidManifest.xml missing permissions: {missing}")
print("AndroidManifest.xml declares USE_FULL_SCREEN_INTENT and REQUEST_IGNORE_BATTERY_OPTIMIZATIONS")
'

  run_py_check 1 "F5" "T1-F5-2" '
files = sorted(pathlib.Path("android/app/src/main").rglob("*.kt"))
file_sources = [strip_comments(f.read_text(), rust=False) for f in files]
joined = "\n".join(file_sources)
has_fg_track = any(k in joined for k in [
    "isAppInForeground", "onAppForegroundChanged", "ProcessLifecycleOwner",
    "ActivityLifecycleCallbacks", "Lifecycle.Event.ON_START", "Lifecycle.Event.ON_STOP",
])
if not has_fg_track:
    sys.exit("missing foreground/background lifecycle tracking for heads-up call notifications")
if "NotificationChannel(" not in joined or "IMPORTANCE_HIGH" not in joined:
    sys.exit("missing high-importance NotificationChannel for incoming call heads-up alert")
notif_sources = [
    s for s in file_sources
    if "NotificationCompat.Builder(" in s and "CATEGORY_CALL" in s and "setFullScreenIntent(" in s
]
if not notif_sources:
    sys.exit("missing NotificationCompat.Builder with CATEGORY_CALL + setFullScreenIntent")
def has_accept_decline_actions(s):
    args_list = []
    for m in re.finditer(r"\baddAction\s*\(", s):
        p_idx = s.find("(", m.start())
        args, _ = extract_delim_expr(s, p_idx, "(", ")", rust=False)
        if args is not None:
            args_list.append(args)
    return any(re.search(r"\bAccept\b", a) for a in args_list) and any(re.search(r"\bDecline\b", a) for a in args_list)

has_call_actions = any(
    "CallStyle.forIncomingCall(" in s or has_accept_decline_actions(s)
    for s in notif_sources
)
if not has_call_actions:
    sys.exit("missing CallStyle.forIncomingCall or Accept/Decline actions on incoming call notification")
print("Background heads-up call notification lifecycle & CallStyle/FullScreenIntent verified")
'

  run_py_check 1 "F5" "T1-F5-3" '
files = sorted(pathlib.Path("android/app/src/main").rglob("*.kt"))
joined = "\n".join(strip_comments(f.read_text(), rust=False) for f in files)
for marker in ["isIgnoringBatteryOptimizations", "ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS"]:
    if marker not in joined:
        sys.exit(f"missing {marker} for battery optimization onboarding")
if "getSharedPreferences(" not in joined or not re.search(r"onboarding_shown|ONBOARDING_SHOWN|battery_onboarding", joined, re.I):
    sys.exit("missing one-time SharedPreferences gating for battery optimization onboarding")
if "com.samsung.android.lool" not in joined or "miui" not in joined.lower():
    sys.exit("missing OEM battery/autostart helper (Samsung Device Care & Xiaomi)")
print("Battery optimization one-time SharedPreferences onboarding & OEM settings helper verified")
'

  run_py_check 1 "F6" "T1-F6-1" '
rel = pathlib.Path(".github/workflows/release.yml").read_text()
if "SquadAir-android-arm64.apk" not in rel or "SHA256SUMS.txt" not in rel:
    sys.exit("release.yml missing SquadAir-android-arm64.apk or SHA256SUMS.txt")
if "SQUADAIR_VERSION" not in rel or "scripts/build_android_apk.sh" not in rel:
    sys.exit("release.yml missing SQUADAIR_VERSION -> build_android_apk.sh wiring")
if not pathlib.Path("scripts/build_android_apk.sh").is_file():
    sys.exit("scripts/build_android_apk.sh missing")
print("release.yml Android APK & SHA256SUMS.txt pipeline contract verified")
'

  run_py_check 1 "F6" "T1-F6-2" '
gradle = strip_comments(pathlib.Path("android/app/build.gradle.kts").read_text(), rust=False)
for req in ["app.squadair.mobile", "minSdk = 26", "targetSdk = 34", "arm64-v8a", "SQUADAIR_VERSION", "buildConfig = true"]:
    if req not in gradle:
        sys.exit(f"build.gradle.kts missing {req}")
manifest = re.sub(r"<!--[\s\S]*?-->", "", pathlib.Path("android/app/src/main/AndroidManifest.xml").read_text())
for req in ["REQUEST_INSTALL_PACKAGES", "${applicationId}.fileprovider"]:
    if req not in manifest:
        sys.exit(f"AndroidManifest.xml missing {req}")
fp = pathlib.Path("android/app/src/main/res/xml/file_paths.xml").read_text()
if "cache-path" not in fp and "files-path" not in fp:
    sys.exit("file_paths.xml missing cache/files path")
kt_joined = "\n".join(strip_comments(f.read_text(), rust=False) for f in sorted(pathlib.Path("android/app/src/main").rglob("*.kt")))
for req in ["alivergg/squad-air/releases/latest", "BuildConfig.VERSION_NAME", "SquadAir-android-arm64.apk", "SHA256SUMS.txt", "canRequestPackageInstalls", "ACTION_MANAGE_UNKNOWN_APP_SOURCES", "FileProvider.getUriForFile"]:
    if req not in kt_joined:
        sys.exit(f"Android Kotlin updater missing {req}")
rs_up = strip_comments(pathlib.Path("crates/squad-client/src/updater.rs").read_text(), rust=True)
for req in ["https://api.github.com/repos/alivergg/squad-air/releases/latest", "SHA256SUMS.txt", "parse_version_tag"]:
    if req not in rs_up:
        sys.exit(f"desktop updater.rs missing {req}")
print("Android & Desktop GitHub updater + FileProvider + install permission contract verified")
'

  run_py_check 1 "F7" "T1-F7-1" '
ui_dir = pathlib.Path("android/app/src/main/java/app/squadair/mobile/ui")
lobby = strip_comments((ui_dir / "LobbyScreen.kt").read_text(), rust=False)
room = strip_comments((ui_dir / "RoomScreen.kt").read_text(), rust=False)

animated_copy_composables = []
for f in sorted(ui_dir.glob("*.kt")):
    src = strip_comments(f.read_text(), rust=False)
    for m in re.finditer(r"\bfun\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(", src):
        fn_name = m.group(1)
        paren_idx = src.find("(", m.start())
        sig, close_paren = extract_delim_expr(src, paren_idx, "(", ")", rust=False)
        body = extract_fn_body(src[m.start():], r"\bfun\s+" + re.escape(fn_name) + r"\s*\(", rust=False)
        if sig is None or body is None:
            continue
        has_set_text = "setText(" in body
        has_check_icon = bool(re.search(r"Icons\.(?:Rounded|Default|Filled|Outlined)\.Check\b", body))
        has_2s_delay = bool(re.search(r"delay\s*\(\s*(?:2000|2_000)\s*[lL]?\s*\)", body))
        has_copied = "\"Copied" in body or "\"Copied" in sig
        if has_set_text and has_check_icon and has_2s_delay and has_copied:
            animated_copy_composables.append(fn_name)

if not animated_copy_composables:
    sys.exit("no animated copy composable (setText + Check icon + delay(2000) + Copied) found in ui/*.kt")

call_pat = re.compile(r"\b(?:" + "|".join(re.escape(n) for n in animated_copy_composables) + r")\s*\(")
lobby_hits = len(call_pat.findall(lobby))
room_hits = len(call_pat.findall(room))
if lobby_hits < 1:
    sys.exit(f"LobbyScreen.kt must invoke animated copy composable >= 1 time (found {lobby_hits})")
if room_hits < 2:
    sys.exit(f"RoomScreen.kt must invoke animated copy composable >= 2 times (found {room_hits})")
print(f"All {lobby_hits + room_hits} Android copy button call sites use animated composables {animated_copy_composables}")
'

  run_py_check 1 "F8" "T1-F8-1" '
src = strip_comments(pathlib.Path("crates/squad-client/src/ui/mod.rs").read_text(), rust=True)

title_body = extract_fn_body(src, r"\bfn\s+title\s*\(", rust=True)
if title_body is None:
    sys.exit("fn title not found in ui/mod.rs")
if "overlay_window_id" not in title_body:
    sys.exit("fn title must check self.overlay_window_id")

title_helpers = []
for helper_name in re.findall(r"\b(overlay_window_[a-z0-9_]+)\s*\(", title_body):
    hb = extract_fn_body(src, r"\bfn\s+" + re.escape(helper_name) + r"\s*\(", rust=True)
    if hb:
        title_helpers.append(hb)

title_combined = "\n".join([title_body] + title_helpers)
if "target_os = \"windows\"" not in title_combined:
    sys.exit("title() must branch on target_os = \"windows\" for overlay window")

def branch_returns_empty(src_text):
    if re.search(r"(?:if\s+(?:is_windows|cfg!\(\s*target_os\s*=\s*\"windows\"\s*\))|#\[cfg\(\s*target_os\s*=\s*\"windows\"\s*\)\])\s*\{\s*return\s+\"\"(?:\.to_string\(\)|\.into\(\))?\s*;?\s*\}", src_text):
        return True
    for m_if in re.finditer(r"\bif\s+(?:is_windows|cfg!\(\s*target_os\s*=\s*\"windows\"\s*\))\s*\{\s*\"\"(?:\.to_string\(\)|\.into\(\))?\s*\}\s*else\s*\{", src_text):
        else_brace = src_text.find("{", m_if.end() - 1)
        _, close_else = extract_delim_expr(src_text, else_brace, "{", "}", rust=True)
        if close_else != -1:
            before = src_text[:m_if.start()].rstrip()
            after = src_text[close_else + 1:].strip()
            if not after or re.search(r"\breturn$", before):
                return True
    for m_cfg in re.finditer(r"#\[cfg\(\s*target_os\s*=\s*\"windows\"\s*\)\]\s*\{\s*\"\"(?:\.to_string\(\)|\.into\(\))?\s*\}", src_text):
        after = src_text[m_cfg.end():].strip()
        if not after:
            return True
        if re.match(r"^#\[cfg\(\s*not\(\s*target_os\s*=\s*\"windows\"\s*\)\s*\)\]\s*\{", after):
            nb = after.find("{")
            _, nc = extract_delim_expr(after, nb, "{", "}", rust=True)
            if nc != -1 and not after[nc + 1:].strip():
                return True
    return False

win_empty_title = any(branch_returns_empty(part) for part in [title_body] + title_helpers)
if not win_empty_title:
    sys.exit("title() must return empty string \"\" on Windows branch")

sync_body = extract_fn_body(src, r"\bfn\s+sync_overlay_window\s*\(", rust=True)
if sync_body is None:
    sys.exit("fn sync_overlay_window not found in ui/mod.rs")

sync_combined = sync_body
for helper_name in re.findall(r"\b(overlay_window_[a-z0-9_]+)\s*\(", sync_body):
    hb = extract_fn_body(src, r"\bfn\s+" + re.escape(helper_name) + r"\s*\(", rust=True)
    if hb:
        sync_combined += "\n" + hb

if "target_os = \"windows\"" not in sync_combined:
    sys.exit("sync_overlay_window() must configure Windows overlay settings")
if re.search(r"\bvisible\s*:\s*true\b", sync_combined):
    sys.exit("sync_overlay_window() must never open overlay with visible: true on Windows")
if not re.search(r"\bvisible\s*:\s*(?:false|!\w+)", sync_combined):
    sys.exit("sync_overlay_window() must set visible: false (or !is_windows) on Windows")
for pat, label in [
    (r"\bdecorations\s*:\s*false\b", "decorations: false"),
    (r"\btransparent\s*:\s*true\b", "transparent: true"),
    (r"\bskip_taskbar\s*=\s*true\b", "skip_taskbar = true"),
    (r"\bundecorated_shadow\s*=\s*false\b", "undecorated_shadow = false"),
]:
    if not re.search(pat, sync_combined):
        sys.exit(f"sync_overlay_window() missing {label}")
print("ui/mod.rs Windows overlay empty title & hidden creation verified")
'

  run_py_check 1 "F8" "T1-F8-2" '
src = strip_comments(pathlib.Path("crates/squad-client/src/ui/overlay_platform.rs").read_text(), rust=True)
cfg_body = extract_fn_body(src, r"\bpub\s+fn\s+configure_overlay_window\s*\(", rust=True)
if cfg_body is None:
    sys.exit("configure_overlay_window not found")

if "enable_mouse_passthrough" in cfg_body:
    guard_spans = []
    for m_cfg in re.finditer(r"#\[cfg\(\s*not\(\s*target_os\s*=\s*\"windows\"\s*\)\s*\)\]", cfg_body):
        pos = m_cfg.end()
        while pos < len(cfg_body) and cfg_body[pos].isspace():
            pos += 1
        if pos < len(cfg_body) and cfg_body[pos] == "{":
            _, close_b = extract_delim_expr(cfg_body, pos, "{", "}", rust=True)
            if close_b != -1:
                guard_spans.append((m_cfg.start(), close_b + 1))
        else:
            d_p = d_b = d_c = 0
            k = pos
            while k < len(cfg_body):
                nk = skip_non_code(cfg_body, k, rust=True)
                if nk != k:
                    k = nk
                    continue
                ch = cfg_body[k]
                if ch == "(":
                    d_p += 1
                elif ch == ")" and d_p > 0:
                    d_p -= 1
                elif ch == "[":
                    d_b += 1
                elif ch == "]" and d_b > 0:
                    d_b -= 1
                elif ch == "{":
                    d_c += 1
                elif ch == "}":
                    if d_c > 0:
                        d_c -= 1
                    else:
                        break
                elif ch == ";" and d_p == 0 and d_b == 0 and d_c == 0:
                    k += 1
                    break
                k += 1
            guard_spans.append((m_cfg.start(), k))
    for m_not_cfg in re.finditer(r"\bif\s+!\s*cfg!\(\s*target_os\s*=\s*\"windows\"\s*\)\s*\{", cfg_body):
        brace_idx = cfg_body.find("{", m_not_cfg.start())
        _, close_b = extract_delim_expr(cfg_body, brace_idx, "{", "}", rust=True)
        if close_b != -1:
            guard_spans.append((brace_idx, close_b + 1))
    for m_help in re.finditer(r"\bif\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*\(\s*cfg!\(\s*target_os\s*=\s*\"windows\"\s*\)\s*\)\s*\{", cfg_body):
        hb = extract_fn_body(src, r"\bfn\s+" + re.escape(m_help.group(1)) + r"\s*\(", rust=True)
        if hb and re.search(r"^\s*!\s*is_windows\s*$", hb.strip()):
            brace_idx = cfg_body.find("{", m_help.start())
            _, close_b = extract_delim_expr(cfg_body, brace_idx, "{", "}", rust=True)
            if close_b != -1:
                guard_spans.append((brace_idx, close_b + 1))
    for m_call in re.finditer(r"\benable_mouse_passthrough\b", cfg_body):
        if not any(gs <= m_call.start() and m_call.end() <= ge for gs, ge in guard_spans):
            sys.exit("configure_overlay_window must guard enable_mouse_passthrough against Windows")

if "DWMNCRP_DISABLED" in src:
    sys.exit("DWMNCRP_DISABLED must be removed from overlay_platform.rs")
if "SetWindowSubclass" not in src and "GWLP_WNDPROC" not in src:
    sys.exit("overlay_platform.rs missing Win32 subclass (SetWindowSubclass / GWLP_WNDPROC)")
for msg_name, hex_code in [
    ("WM_NCCALCSIZE", "0x0083"),
    ("WM_NCACTIVATE", "0x0086"),
    ("WM_NCPAINT", "0x0085"),
    ("WM_STYLECHANGING", "0x007C"),
    ("WM_NCHITTEST", "0x0084"),
]:
    if msg_name not in src and hex_code.lower() not in src.lower():
        sys.exit(f"overlay_platform.rs missing {msg_name} ({hex_code}) handling")
if "SW_SHOWNOACTIVATE" not in src and not ("SWP_SHOWWINDOW" in src and "SWP_NOACTIVATE" in src):
    sys.exit("overlay_platform.rs missing SW_SHOWNOACTIVATE reveal after subclassing")
print("overlay_platform.rs Win32 subclass, no DWMNCRP_DISABLED & SW_SHOWNOACTIVATE verified")
'

  run_py_check 1 "F9" "T1-F9-1" '
icons = pathlib.Path("crates/squad-client/src/ui/icons.rs")
if not icons.exists():
    sys.exit("crates/squad-client/src/ui/icons.rs missing")
icons_src = strip_comments(icons.read_text(), rust=True)
if "canvas" not in icons_src:
    sys.exit("icons.rs must render vector icons via iced::widget::canvas")
mod_src = strip_comments(pathlib.Path("crates/squad-client/src/ui/mod.rs").read_text(), rust=True)
if "mod icons;" not in mod_src:
    sys.exit("crates/squad-client/src/ui/mod.rs missing mod icons;")
cargo = pathlib.Path("Cargo.toml").read_text()
if "\"svg\"" in cargo:
    sys.exit("Cargo.toml must not enable svg feature breaking --locked")
print("ui/icons.rs canvas vector module wired cleanly without Cargo.toml changes")
'

  run_py_check 1 "F9" "T1-F9-2" '
ui_dir = pathlib.Path("crates/squad-client/src/ui")
required_files = ["room.rs", "lobby.rs", "chat.rs", "stream_view.rs", "incoming_call.rs", "mod.rs"]
for fn in required_files:
    text = strip_comments((ui_dir / fn).read_text(), rust=True)
    if not re.search(r"\b(?:with_)?tooltip\s*\(", text):
        sys.exit(f"{fn} missing tooltip(...) / with_tooltip(...) on icon buttons")

control_msgs = [
    "ToggleMicMute", "ToggleDeafen", "OpenSettings", "LeaveRoom",
    "CopyInviteUri", "GenerateShortCode", "CopyShortCode",
    "OpenShareModal", "StopScreenShare", "PickFileToSend",
    "SendChatText", "CopyChatText", "CopyImageToClipboard",
    "ToggleStreamExpanded", "ToggleStreamFullscreen",
    "ExitStreamFullscreen", "StopWatchingStream",
    "RemoveSavedChannel", "RemoveFriend", "CopyMyFriendId",
    "PasteInviteFromClipboard", "ToggleLanHostPasswordVisible",
    "ToggleLanJoinPasswordVisible",
    "SaveActiveChannel", "SaveChannelFromInput",
    "ToggleLocalStreamPreview", "WatchPeerStream", "CallFriend",
    "SaveImageAs", "CloseImageLightbox", "DownloadBlob",
    "OpenDownloadsFolder", "OpenUpdateModal",
]

def find_stmt_end(s, start_idx):
    d_paren = d_brack = d_brace = 0
    i = start_idx
    n = len(s)
    while i < n:
        ch = s[i]
        if ch in "r b" and (i == 0 or not (s[i - 1].isalnum() or s[i - 1] == "_")):
            m_raw = re.match(r"(?:br|rb|r)(#*)\"", s[i:])
            if m_raw:
                end_tok = "\"" + m_raw.group(1)
                end_idx = s.find(end_tok, i + len(m_raw.group(0)))
                if end_idx != -1:
                    i = end_idx + len(end_tok)
                    continue
        if ch == "\"":
            i += 1
            while i < n:
                if s[i] == "\\":
                    i += 2
                    continue
                if s[i] == "\"":
                    i += 1
                    break
                i += 1
            continue
        if ch == "(":
            d_paren += 1
        elif ch == ")":
            if d_paren == 0:
                return i
            d_paren -= 1
        elif ch == "[":
            d_brack += 1
        elif ch == "]":
            if d_brack == 0:
                return i
            d_brack -= 1
        elif ch == "{":
            d_brace += 1
        elif ch == "}":
            if d_brace == 0:
                return i
            d_brace -= 1
        elif ch in (";", ",") and d_paren == 0 and d_brack == 0 and d_brace == 0:
            return i
        i += 1
    return n

def has_enclosing_tooltip(s, btn_start):
    # Walk backwards to check if button(...) is an argument to tooltip( or with_tooltip(
    d_paren = d_brack = d_brace = 0
    i = btn_start - 1
    while i >= 0:
        ch = s[i]
        if ch == "\"":
            i -= 1
            while i >= 0:
                if s[i] == "\"" and (i == 0 or s[i - 1] != "\\"):
                    break
                i -= 1
        elif ch == ")":
            d_paren += 1
        elif ch == "(":
            if d_paren == 0:
                prefix = s[max(0, i - 24):i]
                if re.search(r"\b(?:with_)?tooltip\s*$", prefix):
                    return True
                return False
            d_paren -= 1
        elif ch == "]":
            d_brack += 1
        elif ch == "[":
            if d_brack == 0:
                return False
            d_brack -= 1
        elif ch == "}":
            d_brace += 1
        elif ch == "{":
            if d_brace == 0:
                return False
            d_brace -= 1
        elif ch == ";" and d_paren == 0 and d_brack == 0 and d_brace == 0:
            return False
        i -= 1
    return False

text_violations = []
tooltip_violations = []
for path in sorted(ui_dir.glob("*.rs")):
    clean = strip_comments(path.read_text(), rust=True, mask_chars=True)
    for m in re.finditer(r"\bbutton\s*\(", clean):
        open_idx = clean.find("(", m.start())
        inner, end_idx = extract_delim_expr(clean, open_idx, "(", ")", rust=True)
        if inner is None:
            continue
        stmt_end = find_stmt_end(clean, end_idx + 1)
        chain = clean[end_idx + 1:stmt_end]
        var_name = None
        m_let = re.search(r"let\s+(?:mut\s+)?([a-zA-Z_][a-zA-Z0-9_]*)\s*(?::[^=]+)?=\s*$", clean[max(0, m.start() - 80):m.start()])
        if m_let:
            var_name = m_let.group(1)
            tail_scope = clean[stmt_end:min(len(clean), stmt_end + 1500)]
            for m_var in re.finditer(r"(?:\b" + re.escape(var_name) + r"\s*=\s*)?\b" + re.escape(var_name) + r"\s*\.on_press(?:_with|_maybe)?\s*\(", tail_scope):
                p_idx = tail_scope.find("(", m_var.start())
                on_press_args, _ = extract_delim_expr(tail_scope, p_idx, "(", ")", rust=True)
                if on_press_args is not None:
                    chain += "\n" + on_press_args

        msg_candidates = [cm for cm in control_msgs if not (path.name == "settings.rs" and cm == "OpenUpdateModal")]
        matched_msgs = [cm for cm in msg_candidates if re.search(r"\bMessage::" + re.escape(cm) + r"\b", chain)]
        if not matched_msgs:
            continue

        has_icon = bool(
            re.search(r"\b(?:icons::|icon(?:_colored|_check_animated)?\s*\(|Icon::new\b)", inner)
            or re.search(r"\b(?:code_icon|copy_icon|copy_id_icon|img_copy_icon|invite_copy_icon|invite_content|stop_content)\b", inner)
        )
        has_any_text = bool(re.search(r"\btext\s*\(", inner))
        allowed_non_label_text = (
            has_icon
            and (
                (matched_msgs == ["CopyShortCode"] and "\"" not in inner)
                or (path.name == "lobby.rs" and matched_msgs == ["OpenUpdateModal"])
            )
        )
        if not has_icon or (has_any_text and not allowed_non_label_text):
            line = clean[:m.start()].count("\n") + 1
            text_violations.append(f"{path.name}:{line} ({matched_msgs[0]})")

        has_tip = has_enclosing_tooltip(clean, m.start())
        if not has_tip and var_name:
            tail_scope = clean[stmt_end:min(len(clean), stmt_end + 1500)]
            if re.search(r"\b(?:with_)?tooltip\s*\(\s*" + re.escape(var_name) + r"\b", tail_scope):
                has_tip = True
        if not has_tip:
            line = clean[:m.start()].count("\n") + 1
            tooltip_violations.append(f"{path.name}:{line} ({matched_msgs[0]})")

if text_violations:
    sys.exit("control buttons still using text labels: " + ", ".join(text_violations))
if tooltip_violations:
    sys.exit("control buttons missing enclosing tooltip wrapper: " + ", ".join(tooltip_violations))
print("All desktop control buttons use icons with enclosing tooltips (zero bare/static text labels)")
'

  run_py_check 1 "F10" "T1-F10-1" '
ui_dir = pathlib.Path("crates/squad-client/src/ui")
ui_sources = {p.name: strip_comments(p.read_text(), rust=True) for p in sorted(ui_dir.glob("*.rs"))}
mod_src = ui_sources["mod.rs"]
all_ui = "\n".join(ui_sources.values())

bool_flashes = re.findall(r"\w*copied_flash\s*:\s*bool", mod_src)
if bool_flashes:
    sys.exit(f"found un-expirable bool copy flash fields: {bool_flashes}")
if "copied_msg_id" in all_ui:
    sys.exit("stale copied_msg_id field/usage still present in ui/*.rs")
for fname in ("chat.rs", "mod.rs"):
    if re.search(r"text\s*\(\s*\"Copy Image\"\s*\)", ui_sources[fname]):
        sys.exit(f"{fname} still renders un-animated static \"Copy Image\" text button")

if "Instant" not in all_ui:
    sys.exit("missing Instant timestamp tracking for PC copy flash state")
if "from_secs(2)" not in all_ui and "from_millis(2000)" not in all_ui and "2_000" not in all_ui:
    sys.exit("missing 2-second copy flash reset duration")

update_body = extract_fn_body(mod_src, r"\bpub\s+fn\s+update\s*\(", rust=True)
if update_body is None:
    sys.exit("pub fn update(...) not found in ui/mod.rs")

m_match = re.search(r"\bmatch\s+message\s*\{", update_body)
if not m_match:
    sys.exit("match message { ... } not found in update()")
match_brace = update_body.find("{", m_match.start())
match_body, _ = extract_delim_expr(update_body, match_brace, "{", "}", rust=True)
if match_body is None:
    sys.exit("unclosed match message { ... } in update()")

def parse_match_arms(body):
    arms = []
    n = len(body)
    i = 0
    while i < n:
        while i < n and body[i] in (" ", "\t", "\r", "\n", ","):
            i += 1
        if i >= n:
            break
        pat_start = i
        d_p = d_b = d_c = 0
        arrow_pos = -1
        while i < n:
            ni = skip_non_code(body, i, rust=True)
            if ni != i:
                i = ni
                continue
            ch = body[i]
            if ch == "(":
                d_p += 1
            elif ch == ")" and d_p > 0:
                d_p -= 1
            elif ch == "[":
                d_b += 1
            elif ch == "]" and d_b > 0:
                d_b -= 1
            elif ch == "{":
                d_c += 1
            elif ch == "}" and d_c > 0:
                d_c -= 1
            elif d_p == 0 and d_b == 0 and d_c == 0 and body[i:i + 2] == "=>":
                arrow_pos = i
                break
            i += 1
        if arrow_pos == -1:
            break
        pattern = body[pat_start:arrow_pos].strip()
        i = arrow_pos + 2
        while i < n and body[i] in (" ", "\t", "\r", "\n"):
            i += 1
        expr_start = i
        if i < n and body[i] == "{":
            _, close_b = extract_delim_expr(body, i, "{", "}", rust=True)
            if close_b == -1:
                arms.append((pattern, body[expr_start:]))
                break
            j = close_b + 1
            while j < n and body[j] in (" ", "\t", "\r", "\n"):
                j += 1
            if j < n and body[j] == ".":
                i = close_b + 1
            else:
                arms.append((pattern, body[expr_start:close_b + 1]))
                i = j + 1 if (j < n and body[j] == ",") else j
                continue
        d_p = d_b = d_c = 0
        while i < n:
            ni = skip_non_code(body, i, rust=True)
            if ni != i:
                i = ni
                continue
            ch = body[i]
            if ch == "(":
                d_p += 1
            elif ch == ")" and d_p > 0:
                d_p -= 1
            elif ch == "[":
                d_b += 1
            elif ch == "]" and d_b > 0:
                d_b -= 1
            elif ch == "{":
                d_c += 1
            elif ch == "}" and d_c > 0:
                d_c -= 1
                if d_p == 0 and d_b == 0 and d_c == 0:
                    j = i + 1
                    while j < n and body[j] in (" ", "\t", "\r", "\n"):
                        j += 1
                    if j < n and body[j] == ",":
                        i += 1
                        break
                    if not re.match(r"else\b", body[j:]) and body[j:j + 1] != ".":
                        i += 1
                        break
            elif ch == "," and d_p == 0 and d_b == 0 and d_c == 0:
                break
            i += 1
        arms.append((pattern, body[expr_start:i].strip()))
        if i < n and body[i] == ",":
            i += 1
    return arms

parsed_arms = parse_match_arms(match_body)

def extract_update_arm(msg_name):
    pat = re.compile(r"\bMessage::" + re.escape(msg_name) + r"\b")
    for pattern, expr in parsed_arms:
        if pat.search(pattern):
            return expr
    return None

clear_arm = extract_update_arm("ClearCopiedFlash")
if clear_arm is None or "clear_expired" not in clear_arm:
    sys.exit("Message::ClearCopiedFlash arm missing clear_expired handling in update()")

for msg_name in ["CopyInviteUri", "CopyMyFriendId", "CopyShortCode", "CopyChatText"]:
    arm = extract_update_arm(msg_name)
    if arm is None:
        sys.exit(f"missing Message::{msg_name} arm inside update()")
    if "copy_flash" not in arm or "schedule_copy_flash_tasks" not in arm:
        sys.exit(f"Message::{msg_name} arm inside update() does not trigger copy_flash + 2s timer")

copy_img_arm = extract_update_arm("CopyImageToClipboard")
img_done_arm = extract_update_arm("ImageCopiedToClipboard")
if copy_img_arm is None:
    sys.exit("missing Message::CopyImageToClipboard arm inside update()")
if "copy_flash" not in copy_img_arm:
    if "ImageCopiedToClipboard" not in copy_img_arm or img_done_arm is None or "copy_flash" not in img_done_arm or "schedule_copy_flash_tasks" not in img_done_arm:
        sys.exit("Message::CopyImageToClipboard -> ImageCopiedToClipboard does not trigger copy_flash + 2s timer")

for consumer in ("room.rs", "lobby.rs", "incoming_call.rs", "chat.rs", "mod.rs"):
    if "copy_flash" not in ui_sources[consumer]:
        sys.exit(f"{consumer} does not consume copy_flash animated state")

print("All desktop PC copy buttons & UI consumers use copy_flash Instant timestamps with 2s ClearCopiedFlash expiry")
'

  run_py_check 1 "F11" "T1-F11-1" '
readme = pathlib.Path("README.md").read_text()
for req in ["Android", "SquadAir-android-arm64.apk", "arm64-v8a"]:
    if req not in readme:
        sys.exit(f"README.md missing {req}")
if "Mobile Companion Client (iOS & Android)" in readme:
    sys.exit("README.md Roadmap still claims Android client is unbuilt")
print("README.md Android badges, download table, instructions & roadmap verified")
'

  run_py_check 1 "F11" "T1-F11-2" '
import json
html = pathlib.Path("site/index.html").read_text()
m_ld = re.search(r"<script type=\"application/ld\+json\">([\s\S]*?)</script>", html)
if not m_ld:
    sys.exit("site/index.html missing application/ld+json")
ld = json.loads(m_ld.group(1))
apps = [n for n in ld.get("@graph", []) if n.get("@type") == "SoftwareApplication"]
if not apps or "Android" not in apps[0].get("operatingSystem", ""):
    sys.exit("JSON-LD SoftwareApplication.operatingSystem missing Android")

for marker in ["data-os-switch=\"android\"", "SquadAir-android-arm64.apk", "arm64-v8a"]:
    if marker not in html:
        sys.exit(f"site/index.html missing {marker}")

cards = re.findall(r"<article class=\"install-card[^\"]*\"", html)
if len(cards) < 4:
    sys.exit(f"expected >= 4 .install-card entries in site/index.html, found {len(cards)}")

for asset in ["main.css", "app.js"]:
    mv = re.search(rf"{re.escape(asset)}\?v=(\d+)", html)
    if not mv or int(mv.group(1)) <= 20261121:
        sys.exit(f"site/index.html cache-bust ?v= for {asset} not bumped past 20261121")
print("site/index.html JSON-LD, 4 install cards, Android switcher & cache-bust verified")
'

  run_py_check 1 "F11" "T1-F11-3" '
css = pathlib.Path("site/assets/css/main.css").read_text()
if re.search(r"\.install-grid\s*\{[^}]*repeat\s*\(\s*3\s*,", css):
    sys.exit("main.css .install-grid still uses repeat(3, ...) with 4 install cards")
js = pathlib.Path("site/assets/js/app.js").read_text()
if "android:" not in js or "SquadAir-android-arm64.apk" not in js:
    sys.exit("app.js missing android spec or SquadAir-android-arm64.apk")
llms = pathlib.Path("site/llms.txt").read_text()
if "Android" not in llms:
    sys.exit("site/llms.txt missing Android")
print("site/assets/css/main.css, app.js, and llms.txt Android support verified")
'

  run_py_check 1 "F13" "T1-F13-1" '
import hashlib, subprocess
tracked = subprocess.check_output(["git", "ls-files", "assets/img"], text=True).strip()
if tracked:
    sys.exit(f"assets/img/ must remain untracked, found: {tracked}")
expected = {
    "assets/img/lobby.png": "268451dc678c77d687e4b8b5dd153ed0824800e626faff8e7c5e048809bc9610",
    "assets/img/main.png": "1578e4b912ca6f8e9e288759bb497bde39b7e66d159065cee1f33b542df5a5ba",
}
for path, want in expected.items():
    p = pathlib.Path(path)
    if not p.is_file():
        sys.exit(f"missing {path}")
    got = hashlib.sha256(p.read_bytes()).hexdigest()
    if got != want:
        sys.exit(f"{path} SHA-256 mismatch: expected {want}, got {got}")
print("assets/img/{lobby.png,main.png} untracked and SHA-256 checksums intact")
'

  run_cmd_check 1 "F1,F2,F3" "T1-RUST-E2E" "cargo test --locked -p squad-client --test e2e_requirements -- t1_" \
    cargo test --locked -p squad-client --test e2e_requirements -- t1_
fi

# ==============================================================================
# TIER 2 — Formatting, Cross-Compilation, Clippy & Boundary Tests
# ==============================================================================
if tier_enabled 2; then
  run_cmd_check 2 "F13" "T2-FMT" "cargo fmt --all -- --check" \
    cargo fmt --all -- --check

  run_cmd_check 2 "F9,F13" "T2-CHECK-HOST" "cargo check --workspace --all-targets --locked" \
    cargo check --workspace --all-targets --locked

  run_cmd_check 2 "F1,F4,F6,F13" "T2-CHECK-ANDROID-FFI" "cargo check -p squad-core --no-default-features --features android-ffi --locked" \
    cargo check -p squad-core --no-default-features --features android-ffi --locked

  if feature_enabled "F1,F4,F6,F13"; then
    if command -v rustup >/dev/null 2>&1 && rustup target list --installed 2>/dev/null | grep -qx "aarch64-linux-android"; then
      if NDK_DIR="$(find_android_ndk)"; then
        NDK_BIN=""
        for host_tag in darwin-arm64 darwin-x86_64 linux-x86_64; do
          if [[ -d "${NDK_DIR}/toolchains/llvm/prebuilt/${host_tag}/bin" ]]; then
            NDK_BIN="${NDK_DIR}/toolchains/llvm/prebuilt/${host_tag}/bin"
            break
          fi
        done
        if [[ -n "$NDK_BIN" ]]; then
          run_cmd_check 2 "F1,F4,F6,F13" "T2-CHECK-ANDROID-TARGET" "cargo check -p squad-core --no-default-features --features android-ffi --target aarch64-linux-android --locked" \
            env \
              ANDROID_NDK_HOME="$NDK_DIR" \
              CC_aarch64_linux_android="${NDK_BIN}/aarch64-linux-android26-clang" \
              CXX_aarch64_linux_android="${NDK_BIN}/aarch64-linux-android26-clang++" \
              AR_aarch64_linux_android="${NDK_BIN}/llvm-ar" \
              CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="${NDK_BIN}/aarch64-linux-android26-clang" \
              cargo check -p squad-core --no-default-features --features android-ffi --target aarch64-linux-android --locked
        else
          run_cmd_check 2 "F1,F4,F6,F13" "T2-CHECK-ANDROID-TARGET" "cargo check -p squad-core --no-default-features --features android-ffi --target aarch64-linux-android --locked" \
            cargo check -p squad-core --no-default-features --features android-ffi --target aarch64-linux-android --locked
        fi
      else
        run_cmd_check 2 "F1,F4,F6,F13" "T2-CHECK-ANDROID-TARGET" "cargo check -p squad-core --no-default-features --features android-ffi --target aarch64-linux-android --locked" \
          cargo check -p squad-core --no-default-features --features android-ffi --target aarch64-linux-android --locked
      fi
    elif [[ $OPT_FULL -eq 1 || $OPT_RELEASE_GATE -eq 1 ]]; then
      log_fail 2 "F1,F4,F6,F13" "T2-CHECK-ANDROID-TARGET" "rustup target aarch64-linux-android not installed (required under --full/--release-gate)"
    else
      log_skip 2 "F1,F4,F6,F13" "T2-CHECK-ANDROID-TARGET" "rustup target aarch64-linux-android not installed"
    fi
  fi

  run_cmd_check 2 "F8,F13" "T2-CHECK-WIN" "cargo check --workspace --target x86_64-pc-windows-gnu --locked" \
    cargo check --workspace --target x86_64-pc-windows-gnu --locked

  run_cmd_check 2 "F8,F9,F13" "T2-CLIPPY-HOST" "cargo clippy --workspace --all-targets --locked -- -D warnings" \
    cargo clippy --workspace --all-targets --locked -- -D warnings

  run_cmd_check 2 "F8,F13" "T2-CLIPPY-WIN" "cargo clippy --workspace --target x86_64-pc-windows-gnu --locked -- -D warnings" \
    cargo clippy --workspace --target x86_64-pc-windows-gnu --locked -- -D warnings

  run_cmd_check 2 "F6,F10,F13" "T2-TEST-CLIENT-BINS" "cargo test --locked -p squad-client --bins" \
    cargo test --locked -p squad-client --bins

  run_cmd_check 2 "F1,F2,F3" "T2-RUST-E2E" "cargo test --locked -p squad-client --test e2e_requirements -- t2_" \
    cargo test --locked -p squad-client --test e2e_requirements -- t2_

  if feature_enabled "F4,F5,F6,F7"; then
    if [[ -d "android/app/src/test" ]]; then
      if JAVA_H="$(find_java_home)"; then
        run_cmd_check 2 "F4,F5,F6,F7" "T2-ANDROID-UNIT" "Gradle testReleaseUnitTest" \
          env JAVA_HOME="$JAVA_H" bash -c "cd android && ./gradlew testReleaseUnitTest --no-daemon"
      else
        log_fail 2 "F4,F5,F6,F7" "T2-ANDROID-UNIT" "JAVA_HOME for JDK 17 not found for Gradle unit tests"
      fi
    else
      log_skip 2 "F4,F5,F6,F7" "T2-ANDROID-UNIT" "android/app/src/test does not exist yet"
    fi
  fi
fi

# ==============================================================================
# TIER 3 — Behavioral JS VM, APK Artifact Verification & Multi-Peer Mesh E2E
# ==============================================================================
if tier_enabled 3; then
  if feature_enabled "F11"; then
    if command -v node >/dev/null 2>&1; then
      tmp_js="$(mktemp)"
      if node - <<'NODE' >"$tmp_js" 2>&1
const fs = require("fs");
const vm = require("vm");
const assert = require("assert");

const appCode = fs.readFileSync("site/assets/js/app.js", "utf8");

function runScenario(navCfg) {
  function makeEl(dataset = {}) {
    return {
      textContent: "",
      dataset,
      attrs: {},
      listeners: {},
      classList: { add() {}, remove() {}, toggle() {}, contains() { return false; } },
      setAttribute(k, v) { this.attrs[k] = String(v); },
      getAttribute(k) { return this.attrs[k] ?? null; },
      addEventListener(ev, fn) { (this.listeners[ev] ||= []).push(fn); },
      querySelector() { return null; },
      querySelectorAll() { return []; },
    };
  }

  const labelEl = makeEl();
  const artifactEl = makeEl();
  const heroLabel = makeEl();
  const heroCmd = makeEl();
  const osBtns = ["windows", "macos", "linux", "android"].map((k) => makeEl({ osSwitch: k }));
  const liveEl = makeEl();

  const selectorMap = {
    "[data-download-label]": [labelEl],
    "[data-download-artifact]": [artifactEl],
    "[data-os-switch]": osBtns,
    "[data-hero-install-label]": [heroLabel],
    "[data-hero-install-cmd]": [heroCmd],
  };

  const doc = {
    documentElement: makeEl(),
    body: makeEl(),
    readyState: "complete",
    addEventListener() {},
    getElementById(id) { return id === "live-announce" ? liveEl : null; },
    querySelector(sel) { return (selectorMap[sel] && selectorMap[sel][0]) || null; },
    querySelectorAll(sel) { return selectorMap[sel] || []; },
  };

  const sandbox = {
    window: {
      location: { hash: "" },
      matchMedia: () => ({ matches: false, addEventListener() {} }),
      addEventListener() {},
    },
    document: doc,
    navigator: navCfg,
    IntersectionObserver: undefined,
    localStorage: { getItem() { return null; }, setItem() {} },
    setTimeout,
    clearTimeout,
    console,
  };
  sandbox.window.document = doc;
  sandbox.window.navigator = navCfg;

  vm.runInNewContext(appCode, sandbox, { filename: "site/assets/js/app.js" });
  return { labelEl, artifactEl, heroLabel, heroCmd, osBtns };
}

const scenarios = [
  {
    name: "Android browser with Linux aarch64 platform",
    nav: { platform: "Linux aarch64", userAgent: "Mozilla/5.0 (Linux; Android 14; SM-G973F) AppleWebKit/537.36 Chrome/120.0 Mobile Safari/537.36" },
    expectOs: "android",
    expectArtifact: "SquadAir-android-arm64.apk",
  },
  {
    name: "Chromium UA-CH Android",
    nav: { userAgentData: { platform: "Android" }, platform: "Linux armv8l", userAgent: "" },
    expectOs: "android",
    expectArtifact: "SquadAir-android-arm64.apk",
  },
  {
    name: "Desktop Linux x86_64",
    nav: { platform: "Linux x86_64", userAgent: "Mozilla/5.0 (X11; Linux x86_64) Gecko/20100101 Firefox/121.0" },
    expectOs: "linux",
    expectArtifact: "Snap Store",
  },
  {
    name: "macOS Apple Silicon",
    nav: { platform: "MacIntel", userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)" },
    expectOs: "macos",
    expectArtifact: "SquadAir-Installer-macos-arm64.pkg",
  },
  {
    name: "Windows desktop",
    nav: { platform: "Win32", userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64)" },
    expectOs: "windows",
    expectArtifact: "SquadAir-Setup-windows-x86_64.exe",
  },
];

for (const sc of scenarios) {
  const res = runScenario(sc.nav);
  assert(
    res.artifactEl.textContent.includes(sc.expectArtifact),
    `${sc.name}: expected artifact containing "${sc.expectArtifact}", got "${res.artifactEl.textContent}"`
  );
  const pressed = res.osBtns.find((b) => b.dataset.osSwitch === sc.expectOs);
  assert.strictEqual(pressed.getAttribute("aria-pressed"), "true", `${sc.name}: aria-pressed not true for ${sc.expectOs}`);
}

const winRes = runScenario(scenarios[4].nav);
const androidBtn = winRes.osBtns.find((b) => b.dataset.osSwitch === "android");
assert(androidBtn.listeners.click && androidBtn.listeners.click.length > 0, "android switch missing click handler");
androidBtn.listeners.click[0]();
assert(
  winRes.artifactEl.textContent.includes("SquadAir-android-arm64.apk"),
  `click android button did not switch artifact: "${winRes.artifactEl.textContent}"`
);
assert.strictEqual(androidBtn.getAttribute("aria-pressed"), "true");
console.log("All 5 browser UA/platform detection scenarios + android click switch passed");
NODE
      then
        log_pass 3 "F11" "T3-F11-JS" "$(cat "$tmp_js")"
      else
        log_fail 3 "F11" "T3-F11-JS" "$(cat "$tmp_js" | tr '\n' ' ')"
      fi
      rm -f "$tmp_js"
    else
      log_fail 3 "F11" "T3-F11-JS" "node binary not found"
    fi
  fi

  APK_PATH="dist/SquadAir-android-arm64.apk"
  if feature_enabled "F4,F5,F6,F13"; then
    if [[ ! -f "$APK_PATH" ]]; then
      log_fail 3 "F6,F13" "T3-APK-EXISTS" "$APK_PATH does not exist"
    elif ! BT_DIR="$(find_build_tools)"; then
      log_fail 3 "F6,F13" "T3-APK-TOOLS" "Android SDK build-tools aapt2 not found under Android SDK build-tools"
    else
      BADGING_OUT="$("${BT_DIR}/aapt2" dump badging "$APK_PATH" 2>&1 || true)"
      if echo "$BADGING_OUT" | grep -q "package: name='app.squadair.mobile'" \
        && echo "$BADGING_OUT" | grep -q "sdkVersion:'26'" \
        && echo "$BADGING_OUT" | grep -q "targetSdkVersion:'34'"; then
        missing_perms=""
        for perm in \
          android.permission.RECORD_AUDIO \
          android.permission.MODIFY_AUDIO_SETTINGS \
          android.permission.POST_NOTIFICATIONS \
          android.permission.FOREGROUND_SERVICE \
          android.permission.FOREGROUND_SERVICE_MICROPHONE \
          android.permission.FOREGROUND_SERVICE_MEDIA_PLAYBACK \
          android.permission.USE_FULL_SCREEN_INTENT \
          android.permission.REQUEST_IGNORE_BATTERY_OPTIMIZATIONS \
          android.permission.REQUEST_INSTALL_PACKAGES; do
          if ! echo "$BADGING_OUT" | grep -q "uses-permission: name='${perm}'"; then
            missing_perms="${missing_perms} ${perm}"
          fi
        done
        if [[ -z "$missing_perms" ]]; then
          log_pass 3 "F4,F5,F6,F13" "T3-APK-BADGING" "aapt2 badging & all 9 required permissions verified"
        else
          log_fail 3 "F4,F5,F6,F13" "T3-APK-BADGING" "APK missing permissions:${missing_perms}"
        fi
      else
        log_fail 3 "F4,F5,F6,F13" "T3-APK-BADGING" "unexpected aapt2 badging output"
      fi

      MANIFEST_TREE="$("${BT_DIR}/aapt2" dump xmltree "$APK_PATH" --file AndroidManifest.xml 2>&1 || true)"
      FP_RES="$("${BT_DIR}/aapt2" dump resources "$APK_PATH" 2>/dev/null | awk '/xml\/file_paths/{f=1} f && /\(file\)/{print $3; exit}' || true)"
      if echo "$MANIFEST_TREE" | grep -q 'app.squadair.mobile.fileprovider' \
        && echo "$MANIFEST_TREE" | grep -q 'grantUriPermissions.*=true' \
        && [[ -n "$FP_RES" ]]; then
        FP_TREE="$("${BT_DIR}/aapt2" dump xmltree "$APK_PATH" --file "$FP_RES" 2>&1 || true)"
        if echo "$FP_TREE" | grep -q 'cache-path' && echo "$FP_TREE" | grep -q 'external-files-path'; then
          log_pass 3 "F6,F13" "T3-APK-FILEPROVIDER" "FileProvider authority & resolved $FP_RES verified"
        else
          log_fail 3 "F6,F13" "T3-APK-FILEPROVIDER" "resolved $FP_RES missing cache-path or external-files-path"
        fi
      else
        log_fail 3 "F6,F13" "T3-APK-FILEPROVIDER" "FileProvider authority or xml/file_paths missing in APK"
      fi

      ZIP_LIST="$(unzip -l "$APK_PATH" 2>&1 || true)"
      ABI_OK=1
      for lib in lib/arm64-v8a/libsquad_core.so lib/arm64-v8a/libc++_shared.so lib/arm64-v8a/libjnidispatch.so; do
        if ! echo "$ZIP_LIST" | grep -q "$lib"; then
          ABI_OK=0
        fi
      done
      if echo "$ZIP_LIST" | grep -qE 'lib/(armeabi-v7a|x86|x86_64)/'; then
        ABI_OK=0
      fi
      if [[ $ABI_OK -eq 1 ]]; then
        if JAVA_H="$(find_java_home)" && JAVA_HOME="$JAVA_H" "${BT_DIR}/apksigner" verify --verbose "$APK_PATH" >/dev/null 2>&1; then
          log_pass 3 "F6,F13" "T3-APK-SIGN-ABI" "arm64-v8a-only native libs & apksigner v2/v3 signature verified"
        else
          log_fail 3 "F6,F13" "T3-APK-SIGN-ABI" "apksigner verification failed"
        fi
      else
        log_fail 3 "F6,F13" "T3-APK-SIGN-ABI" "APK native ABI check failed (expected arm64-v8a libs only)"
      fi
    fi
  fi

  run_cmd_check 3 "F1,F2,F3" "T3-RUST-E2E" "cargo test --locked -p squad-client --test e2e_requirements -- t3_" \
    cargo test --locked -p squad-client --test e2e_requirements -- t3_
fi

# ==============================================================================
# TIER 4 — Real-World Workloads, APK Build, Live S10 ADB & Release Gate
# ==============================================================================
if tier_enabled 4; then
  run_cmd_check 4 "F1,F2,F3" "T4-RUST-E2E" "cargo test --locked -p squad-client --test e2e_requirements -- t4_" \
    cargo test --locked -p squad-client --test e2e_requirements -- t4_

  run_cmd_check 4 "F1,F2,F3,F6,F8,F9,F10,F13" "T4-RUST-WS" "cargo test --workspace --locked" \
    cargo test --workspace --locked

  if [[ $OPT_BUILD_APK -eq 1 ]] && feature_enabled "F4,F5,F6,F7,F13"; then
    run_cmd_check 4 "F13" "T4-APK-BUILD" "./scripts/build_android_apk.sh" \
      ./scripts/build_android_apk.sh
  fi

  if [[ $OPT_ADB -eq 1 ]] && feature_enabled "F4,F5,F6,F7,F13"; then
    ADB_CONN_OK=0
    if ! command -v adb >/dev/null 2>&1; then
      if [[ $OPT_REQUIRE_ADB -eq 1 ]]; then
        log_fail 4 "F13" "T4-ADB-CONN" "adb command not found (--require-adb set)"
      else
        log_skip 4 "F13" "T4-ADB-CONN" "adb command not found"
      fi
    else
      ADB_STATE="$(adb get-state 2>&1 || true)"
      if [[ "$ADB_STATE" != "device" ]]; then
        if [[ $OPT_REQUIRE_ADB -eq 1 ]]; then
          log_fail 4 "F13" "T4-ADB-CONN" "no authorized ADB device ($ADB_STATE)"
        else
          log_skip 4 "F13" "T4-ADB-CONN" "ADB device unavailable ($ADB_STATE)"
        fi
      else
        DEV_ABI="$(adb shell getprop ro.product.cpu.abi 2>&1 | tr -d '\r' || true)"
        DEV_SDK="$(adb shell getprop ro.build.version.sdk 2>&1 | tr -d '\r' || true)"
        if [[ "$DEV_ABI" == "arm64-v8a" && "$DEV_SDK" =~ ^[0-9]+$ && "$DEV_SDK" -ge 26 ]]; then
          ADB_CONN_OK=1
          log_pass 4 "F13" "T4-ADB-CONN" "authorized arm64-v8a device connected (SDK $DEV_SDK)"
        else
          log_fail 4 "F13" "T4-ADB-CONN" "unexpected device ABI='$DEV_ABI' SDK='$DEV_SDK'"
        fi
      fi
    fi

    if [[ $ADB_CONN_OK -eq 1 ]]; then
      if [[ $OPT_INSTALL_ADB -eq 1 ]]; then
        run_cmd_check 4 "F13" "T4-ADB-INSTALL" "adb install -r dist/SquadAir-android-arm64.apk" \
          adb install -r dist/SquadAir-android-arm64.apk
      fi

      PKG_DUMP="$(adb shell dumpsys package app.squadair.mobile 2>&1 || true)"
      if [[ -f "dist/SquadAir-android-arm64.apk" ]] && BT_DIR="$(find_build_tools)"; then
        APK_BADGE="$("${BT_DIR}/aapt2" dump badging dist/SquadAir-android-arm64.apk 2>/dev/null || true)"
        APK_VC="$(echo "$APK_BADGE" | sed -n "s/.*versionCode='\([0-9]*\)'.*/\1/p")"
        APK_VN="$(echo "$APK_BADGE" | sed -n "s/.*versionName='\([^']*\)'.*/\1/p")"
        DEV_VC="$(echo "$PKG_DUMP" | sed -n 's/.*versionCode=\([0-9]*\).*/\1/p' | head -n1)"
        DEV_VN="$(echo "$PKG_DUMP" | sed -n 's/.*versionName=\([^[:space:]]*\).*/\1/p' | head -n1)"
        if [[ -n "$APK_VC" && "$APK_VC" == "$DEV_VC" && "$APK_VN" == "$DEV_VN" ]]; then
          log_pass 4 "F13" "T4-ADB-PARITY" "installed APK parity verified (versionCode=$DEV_VC versionName=$DEV_VN)"
        else
          log_fail 4 "F13" "T4-ADB-PARITY" "APK ($APK_VC/$APK_VN) != installed ($DEV_VC/$DEV_VN)"
        fi
      fi

      if echo "$PKG_DUMP" | grep -q "app.squadair.mobile.fileprovider"; then
        log_pass 4 "F6" "T4-ADB-FILEPROVIDER" "app.squadair.mobile.fileprovider registered on device"
      else
        log_fail 4 "F6" "T4-ADB-FILEPROVIDER" "FileProvider authority missing from dumpsys package"
      fi

      APPOPS="$(adb shell cmd appops get app.squadair.mobile 2>&1 || true)"
      if echo "$APPOPS" | grep -q "RECORD_AUDIO: allow"; then
        log_pass 4 "F4" "T4-ADB-APPOPS" "RECORD_AUDIO allowed; appops=$(echo "$APPOPS" | grep -E 'POST_NOTIFICATION|REQUEST_INSTALL_PACKAGES' | tr '\n' ' ' || true)"
      else
        log_fail 4 "F4" "T4-ADB-APPOPS" "RECORD_AUDIO not allowed: $APPOPS"
      fi

      WHITELIST="$(adb shell dumpsys deviceidle whitelist 2>&1 || true)"
      BUCKET="$(adb shell am get-standby-bucket app.squadair.mobile 2>&1 | tr -d '\r' || true)"
      if echo "$WHITELIST" | grep -q "app.squadair.mobile"; then
        log_pass 4 "F5" "T4-ADB-BATTERY" "deviceidle whitelisted; standby-bucket=$BUCKET"
      else
        log_fail 4 "F5" "T4-ADB-BATTERY" "not in deviceidle whitelist (standby-bucket=$BUCKET)"
      fi

      NOTIF="$(adb shell dumpsys notification --noredact 2>&1 | grep -A 30 "app.squadair.mobile" || true)"
      if [[ -n "$NOTIF" ]]; then
        log_pass 4 "F5" "T4-ADB-NOTIF" "notification state captured (channels present)"
      else
        log_fail 4 "F5" "T4-ADB-NOTIF" "no notification record found for app.squadair.mobile"
      fi

      AUDIO_DUMP="$(adb shell dumpsys audio 2>&1 | grep -E "Active communication device|Computed Preferred|Applied Preferred|mCommunicationStrategyId" | tr '\n' ';' || true)"
      FLINGER_DUMP="$(adb shell dumpsys media.audio_flinger 2>&1 | grep -E "AudioOut_|AudioIn_|Output devices|Input device" | head -n 10 | tr '\n' ';' || true)"
      LOGCAT_ERR="$(adb logcat -d -t 200 2>&1 | grep -E "AAudioStream.*ERROR|AAudio.*DISCONNECT|AudioRoute.*FATAL|Oboe.*Error" || true)"
      if [[ -n "$AUDIO_DUMP" && -n "$FLINGER_DUMP" && -z "$LOGCAT_ERR" ]]; then
        log_pass 4 "F4" "T4-ADB-AUDIO" "dumpsys audio & media.audio_flinger inspected cleanly"
      else
        log_fail 4 "F4" "T4-ADB-AUDIO" "audio dump or logcat error detected ($LOGCAT_ERR)"
      fi
    fi
  fi

  if [[ $OPT_RELEASE_GATE -eq 1 ]] && feature_enabled "F11,F13"; then
    BRANCH="$(git rev-parse --abbrev-ref HEAD)"
    APK_TRACKED="$(git ls-files dist/SquadAir-android-arm64.apk)"
    if [[ "$BRANCH" == "main" && "$APK_TRACKED" == "dist/SquadAir-android-arm64.apk" ]]; then
      log_pass 4 "F13" "T4-GIT-MAIN" "on branch main with tracked dist/SquadAir-android-arm64.apk"
    else
      log_fail 4 "F13" "T4-GIT-MAIN" "branch='$BRANCH' tracked_apk='$APK_TRACKED' (expected main + tracked APK)"
    fi
  fi
fi

echo "================================================================================"
echo "Summary: PASS=${PASS_COUNT} FAIL=${FAIL_COUNT} SKIP=${SKIP_COUNT}"
echo "================================================================================"

if [[ $FAIL_COUNT -gt 0 ]]; then
  exit 1
fi
exit 0
