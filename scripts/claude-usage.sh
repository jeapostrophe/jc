#!/bin/bash
# claude-usage.sh — Claude Code usage par report with caching.
# Standalone script: requires curl, jq, and macOS Keychain with Claude Code credentials.
#
# Usage:
#   ./claude-usage.sh                    # full colored output, one block per model
#   ./claude-usage.sh --model opus       # full output for a single model
#   ./claude-usage.sh --line             # single-line (most constrained model)
#   ./claude-usage.sh --line --model opus # single-line for a specific model
#
# Cache: ~/.cache/claude-usage.json (refreshed every 5 minutes)
#
# Config: ~/.config/claude-usage.conf (optional, sourced if present)
#   Working hours as bash arrays — each is (start_hour end_hour), 24h clock.
#   Set (0 0) for a day off. Example:
#
#     MON=(9 17)
#     TUE=(9 17)
#     WED=(9 17)
#     THU=(9 17)
#     FRI=(9 17)
#     SAT=(0 0)
#     SUN=(0 0)
#     CACHE_MAX_AGE=300
#
# Defaults (9-17 weekdays, weekends off):
MON=(9 17); TUE=(9 17); WED=(9 17); THU=(9 17); FRI=(9 17); SAT=(0 0); SUN=(0 0)

CACHE_FILE="${HOME}/.cache/claude-usage.json"
CACHE_MAX_AGE=300  # seconds
KEYCHAIN_SERVICE="Claude Code-credentials"
USAGE_URL="https://api.anthropic.com/api/oauth/usage"
THRESHOLD=5

# Source user config (can override any variable above)
CONF="${HOME}/.config/claude-usage.conf"
[[ -f "$CONF" ]] && source "$CONF"

# --- helpers ---

die() { echo "error: $1" >&2; exit 1; }

# Detect date flavor once
if date -j -f "%s" 0 +%s >/dev/null 2>&1; then
  DATE_FLAVOR="bsd"
elif date -d "@0" +%s >/dev/null 2>&1; then
  DATE_FLAVOR="gnu"
else
  die "unsupported date command (need BSD or GNU date)"
fi

epoch() {
  date +%s
}

# Parse ISO 8601 to epoch seconds (handles both BSD and GNU date)
iso_to_epoch() {
  local iso="$1"
  if [[ "$DATE_FLAVOR" == "gnu" ]]; then
    date -d "$iso" +%s 2>/dev/null
  else
    # BSD date: strip sub-second precision, normalize timezone format
    local cleaned
    cleaned=$(echo "$iso" | sed -E 's/\.[0-9]+//' | sed 's/Z/+0000/' | sed -E 's/([+-][0-9]{2}):([0-9]{2})$/\1\2/')
    date -j -f "%Y-%m-%dT%H:%M:%S%z" "$cleaned" +%s 2>/dev/null
  fi
}

# Day of week (0=Sun, 1=Mon, ..., 6=Sat) from epoch
dow_from_epoch() {
  if [[ "$DATE_FLAVOR" == "gnu" ]]; then
    date -d "@$1" +%w 2>/dev/null
  else
    date -j -f "%s" "$1" +%w 2>/dev/null
  fi
}

# Start of day (midnight) from epoch
midnight_from_epoch() {
  local datestr
  if [[ "$DATE_FLAVOR" == "gnu" ]]; then
    datestr=$(date -d "@$1" +"%Y-%m-%d")
    date -d "$datestr" +%s 2>/dev/null
  else
    datestr=$(date -j -f "%s" "$1" +"%Y-%m-%d")
    date -j -f "%Y-%m-%d" "$datestr" +%s 2>/dev/null
  fi
}

# Format epoch as "Thu 21:00"
weekday_time_from_epoch() {
  if [[ "$DATE_FLAVOR" == "gnu" ]]; then
    date -d "@$1" +"%a %H:%M" 2>/dev/null
  else
    date -j -f "%s" "$1" +"%a %H:%M" 2>/dev/null
  fi
}

# Get work hours [start, end] for a given dow (0=Sun..6=Sat)
work_hours_for_dow() {
  case "$1" in
    0) echo "${SUN[@]}" ;;
    1) echo "${MON[@]}" ;;
    2) echo "${TUE[@]}" ;;
    3) echo "${WED[@]}" ;;
    4) echo "${THU[@]}" ;;
    5) echo "${FRI[@]}" ;;
    6) echo "${SAT[@]}" ;;
  esac
}

# --- caching ---

cache_is_fresh() {
  [[ -f "$CACHE_FILE" ]] || return 1
  local now file_mtime age
  now=$(epoch)
  if [[ "$DATE_FLAVOR" == "gnu" ]]; then
    file_mtime=$(stat -c %Y "$CACHE_FILE" 2>/dev/null) || return 1
  else
    file_mtime=$(stat -f %m "$CACHE_FILE" 2>/dev/null) || return 1
  fi
  age=$((now - file_mtime))
  (( age < CACHE_MAX_AGE ))
}

fetch_and_cache() {
  local token json
  token=$(security find-generic-password -s "$KEYCHAIN_SERVICE" -w 2>/dev/null) \
    || die "failed to read keychain (is Claude Code signed in?)"
  local access_token
  access_token=$(echo "$token" | jq -r '.claudeAiOauth.accessToken // empty') \
    || die "failed to parse keychain JSON"
  [[ -n "$access_token" ]] || die "no accessToken in keychain credentials"

  mkdir -p "$(dirname "$CACHE_FILE")"
  json=$(curl -s -f -H "Authorization: Bearer $access_token" \
    -H "anthropic-beta: oauth-2025-04-20" "$USAGE_URL") \
    || die "API request failed"
  echo "$json" > "$CACHE_FILE"
}

# --- par calculation ---

# Compute par stats for a given utilization % and reset ISO timestamp.
# Sets these variables in the caller's scope:
#   par, pace, remaining, working_pct, limit_pct (overwritten with input),
#   seven_rel, window_end
compute_par() {
  local _limit="$1" _reset_iso="$2"
  local now_epoch
  now_epoch=$(epoch)

  limit_pct="$_limit"

  local window_end_epoch window_start
  window_end_epoch=$(iso_to_epoch "$_reset_iso")
  [[ -n "$window_end_epoch" ]] || die "failed to parse reset time: $_reset_iso"
  window_end="$window_end_epoch"
  window_start=$((window_end_epoch - 7 * 86400))

  # Iterate days in window, accumulate working seconds
  local total_work=0 elapsed_work=0
  for i in $(seq 0 7); do
    local day_start=$((window_start + i * 86400))
    local day_midnight
    day_midnight=$(midnight_from_epoch "$day_start")
    [[ -n "$day_midnight" ]] || continue
    local dow
    dow=$(dow_from_epoch "$day_start")

    local hours
    hours=($(work_hours_for_dow "$dow"))
    local ws=${hours[0]:-0} we=${hours[1]:-0}

    (( ws >= we )) && continue

    local work_start=$((day_midnight + ws * 3600))
    local work_end=$((day_midnight + we * 3600))

    (( work_start < window_start )) && work_start=$window_start
    (( work_end > window_end_epoch )) && work_end=$window_end_epoch
    (( work_start >= work_end )) && continue

    total_work=$((total_work + work_end - work_start))

    local elapsed_end=$work_end
    (( elapsed_end > now_epoch )) && elapsed_end=$now_epoch
    if (( work_start < elapsed_end )); then
      elapsed_work=$((elapsed_work + elapsed_end - work_start))
    fi
  done

  working_pct=0
  if (( total_work > 0 )); then
    working_pct=$(awk "BEGIN { printf \"%.1f\", $elapsed_work / $total_work * 100 }")
  fi

  par=$(awk "BEGIN { printf \"%.0f\", $working_pct - $limit_pct }")
  if awk "BEGIN { exit ($working_pct > 0) ? 0 : 1 }"; then
    pace=$(awk "BEGIN { printf \"%.1f\", $limit_pct / $working_pct }")
  else
    pace="—"
  fi

  local elapsed_hours
  elapsed_hours=$(awk "BEGIN { printf \"%.2f\", $elapsed_work / 3600 }")
  if awk "BEGIN { exit ($elapsed_hours > 0 && $limit_pct > 0) ? 0 : 1 }"; then
    remaining=$(awk "BEGIN {
      burn = $limit_pct / $elapsed_hours
      rem = (100 - $limit_pct) / burn
      if (rem > 99) printf \">99h\"
      else printf \"%.1fh\", rem
    }")
  else
    remaining="—"
  fi

  seven_rel=$(weekday_time_from_epoch "$window_end_epoch" || echo "?")
}

# Resolve which 7-day window to use. Sets limit_pct and seven_reset_iso.
# Args: $1=json, $2=model filter (empty = best overall)
resolve_seven_day() {
  local json="$1" model="$2"
  limit_pct="" seven_reset_iso=""

  if [[ -n "$model" ]]; then
    local model_lower
    model_lower=$(echo "$model" | tr '[:upper:]' '[:lower:]')
    eval "$(echo "$json" | jq -r --arg m "$model_lower" '
      [to_entries[]
       | select(.key | startswith("seven_day_"))
       | select(.value != null and .value.resets_at != null)
       | select(.key | ltrimstr("seven_day_") | inside($m))]
      | sort_by(-.value.utilization) | first // empty
      | "limit_pct=\(.value.utilization)\nseven_reset_iso=\(.value.resets_at)"
    ')"
  fi

  # Only fall back to aggregate when no specific model was requested
  if [[ -z "$seven_reset_iso" || "$seven_reset_iso" == "null" ]] && [[ -z "$model" ]]; then
    eval "$(echo "$json" | jq -r '
      [to_entries[] | select(.key | startswith("seven_day")) | select(.value != null and .value.resets_at != null) | .value]
      | sort_by(-.utilization) | first // empty
      | "limit_pct=\(.utilization)\nseven_reset_iso=\(.resets_at)"
    ')"
  fi

  [[ -z "$seven_reset_iso" || "$seven_reset_iso" == "null" ]] && return 1
  return 0
}

# List model keys that have valid 7-day windows (e.g. "opus sonnet").
list_model_keys() {
  echo "$1" | jq -r '
    [to_entries[]
     | select(.key | startswith("seven_day_"))
     | select(.value != null and .value.resets_at != null)
     | (.key | ltrimstr("seven_day_"))]
    | sort | .[]
  '
}

format_five_hour() {
  local json="$1"
  local now_epoch
  now_epoch=$(epoch)

  local five_pct reset_iso
  five_pct=$(echo "$json" | jq -r '.five_hour.utilization // 0')
  reset_iso=$(echo "$json" | jq -r '.five_hour.resets_at // empty')

  local five_rel="?"
  local five_reset_epoch
  five_reset_epoch=$(iso_to_epoch "$reset_iso")
  if [[ -n "$five_reset_epoch" ]]; then
    local five_diff=$((five_reset_epoch - now_epoch))
    if (( five_diff <= 0 )); then
      five_rel="now"
    else
      local h=$((five_diff / 3600)) m=$(( (five_diff % 3600) / 60 ))
      if (( h > 0 )); then
        five_rel="in ${h}h $(printf '%02d' $m)m"
      else
        five_rel="in ${m}m"
      fi
    fi
  fi

  local red=$'\e[31m' green=$'\e[32m' yellow=$'\e[33m' dim=$'\e[2m' reset=$'\e[0m'
  local five_color="$green"
  awk "BEGIN { exit ($five_pct > 80) ? 0 : 1 }" && five_color="$red"
  awk "BEGIN { exit ($five_pct > 50 && $five_pct <= 80) ? 0 : 1 }" && five_color="$yellow"
  local five_int
  five_int=$(printf '%.0f' "$five_pct")
  printf "  5h window: ${five_color}%3s%%${reset} %s ${dim}resets %s${reset}\n" \
    "$five_int" "$(bar "$five_pct" "$five_color" "$dim" "$reset")" "$five_rel"
}

# Render one model block. Expects compute_par to have been called already.
# Args: $1=label (e.g. "Sonnet" or empty for unlabeled)
render_block() {
  local label="$1"
  local red=$'\e[31m' green=$'\e[32m' yellow=$'\e[33m' dim=$'\e[2m' bold=$'\e[1m' reset=$'\e[0m'

  local par_color par_label sign=""
  if awk "BEGIN { exit ($par > $THRESHOLD) ? 0 : 1 }"; then
    par_color="$green"; par_label="Under par"; sign="+"
  elif awk "BEGIN { exit ($par < -$THRESHOLD) ? 0 : 1 }"; then
    par_color="$red"; par_label="Over par"
  else
    par_color="$yellow"; par_label="On par"
    awk "BEGIN { exit ($par > 0) ? 0 : 1 }" && sign="+"
  fi

  local heading="${par_label}: ${sign}${par}"
  if [[ -n "$label" ]]; then
    heading="${label}: ${sign}${par} (${par_label})"
  fi
  echo "  ${bold}${par_color}${heading}${reset}"

  local limit_int working_int
  limit_int=$(printf '%.0f' "$limit_pct")
  working_int=$(printf '%.0f' "$working_pct")

  local working_color="$yellow"
  awk "BEGIN { exit ($working_pct > $limit_pct + $THRESHOLD) ? 0 : 1 }" && working_color="$green"
  awk "BEGIN { exit ($working_pct < $limit_pct - $THRESHOLD) ? 0 : 1 }" && working_color="$red"

  printf "  7d budget: %3s%% %s ${dim}resets %s${reset}\n" \
    "$limit_int" "$(bar "$limit_pct" "$dim" "$dim" "$reset")" "$seven_rel"
  printf "  Work time: ${working_color}%3s%%${reset} %s\n" \
    "$working_int" "$(bar "$working_pct" "$working_color" "$dim" "$reset")"

  local pace_color="$yellow"
  if [[ "$pace" != "—" ]]; then
    awk "BEGIN { exit ($pace > 1.1) ? 0 : 1 }" && pace_color="$red"
    awk "BEGIN { exit ($pace < 0.9) ? 0 : 1 }" && pace_color="$green"
  fi
  echo "       Pace: ${pace_color}${pace}x${reset}  ${dim}Remaining: ${remaining}${reset}"
}

render_extra_usage() {
  local json="$1"
  local dim=$'\e[2m' reset=$'\e[0m'
  local extra_enabled
  extra_enabled=$(echo "$json" | jq -r '.extra_usage.is_enabled // false')
  if [[ "$extra_enabled" == "true" ]]; then
    local monthly used util
    monthly=$(echo "$json" | jq -r '.extra_usage.monthly_limit // 0')
    used=$(echo "$json" | jq -r '.extra_usage.used_credits // 0')
    util=$(echo "$json" | jq -r '.extra_usage.utilization // 0')
    printf "  ${dim}Extra usage: \$%.0f / \$%.0f (%.1f%%)${reset}\n" "$used" "$monthly" "$util"
  fi
}

# --- output modes ---

output_line() {
  local json="$1"
  resolve_seven_day "$json" "$MODEL_FILTER" || return
  compute_par "$limit_pct" "$seven_reset_iso"

  local sign=""
  (( $(echo "$par" | tr -d '-') != 0 )) && { awk "BEGIN { exit ($par > 0) ? 0 : 1 }" && sign="+"; }
  echo "Par ${sign}${par} | ${pace}x | ${remaining}"
}

output_single() {
  local json="$1"
  if ! resolve_seven_day "$json" "$MODEL_FILTER"; then
    local dim=$'\e[2m' reset=$'\e[0m'
    echo "  ${dim}No usage data for model: ${MODEL_FILTER}${reset}"
    echo ""
    format_five_hour "$json"
    return
  fi
  compute_par "$limit_pct" "$seven_reset_iso"

  render_block ""
  echo ""
  format_five_hour "$json"
  echo ""
  render_extra_usage "$json"
}

output_all() {
  local json="$1"
  local keys
  keys=$(list_model_keys "$json")

  # 5-hour window (shared across models)
  format_five_hour "$json"
  echo ""

  # One block per model
  local first=true
  for key in $keys; do
    local util reset_iso
    eval "$(echo "$json" | jq -r --arg k "seven_day_${key}" '
      .[$k] | "util=\(.utilization)\nreset_iso=\(.resets_at)"
    ')"

    $first || echo ""
    first=false

    local label
    label="$(echo "${key:0:1}" | tr '[:lower:]' '[:upper:]')${key:1}"
    compute_par "$util" "$reset_iso"
    render_block "$label"
  done

  # If no model-specific keys, fall back to aggregate
  if [[ -z "$keys" ]]; then
    resolve_seven_day "$json" ""
    compute_par "$limit_pct" "$seven_reset_iso"
    render_block ""
  fi

  echo ""
  render_extra_usage "$json"
}

bar() {
  local pct="$1" fill="$2" empty_style="$3" reset="$4"
  local width=40
  local filled
  filled=$(awk "BEGIN { printf \"%d\", ($pct / 100) * $width + 0.5 }")
  local empty=$((width - filled))
  local bar_fill="" bar_empty=""
  for ((i=0; i<filled; i++)); do bar_fill+="█"; done
  for ((i=0; i<empty; i++)); do bar_empty+="░"; done
  echo "${fill}${bar_fill}${empty_style}${bar_empty}${reset}"
}

# --- main ---

MODE=""
MODEL_FILTER=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --line) MODE="line"; shift ;;
    --model) MODEL_FILTER="$2"; shift 2 ;;
    *) shift ;;
  esac
done

if ! cache_is_fresh; then
  fetch_and_cache
fi

json=$(cat "$CACHE_FILE")

if [[ "$MODE" == "line" ]]; then
  output_line "$json"
elif [[ -n "$MODEL_FILTER" ]]; then
  output_single "$json"
else
  output_all "$json"
fi
