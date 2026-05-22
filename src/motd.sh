#!/bin/sh
# /etc/profile.d/qvm-motd.sh — installed by qvm cloud-init.
#
# Pure POSIX sh, feature-detected. Every external tool is guarded by
# `command -v` so missing utilities (lastlog on Alpine, ifconfig on
# busybox, etc.) silently drop the corresponding row instead of erroring.
#
# Colour palette + default mode are baked in below. `qvm` overwrites
# them by literal string-replace before the script is dropped into the
# cloud-init seed. Each *_ESC value is the ANSI sequence MINUS its ESC
# prefix (so the file is grep-friendly + diff-friendly).

# ── configurable knobs (qvm rewrites these lines) ───────────────────
COLOR_MODE_DEFAULT="auto"   # auto | always | never
LABEL_ESC='[0;36m'          # field labels (cyan)
BOLD_ESC='[1m'              # hostname banner (bold)
OK_ESC='[0;32m'             # CPU/RAM below 60% (green)
WARN_ESC='[1;33m'           # 60–79% (yellow)
CRIT_ESC='[0;31m'           # ≥ 80% (red)
# ────────────────────────────────────────────────────────────────────

COLOR_MODE="${COLOR_MODE:-$COLOR_MODE_DEFAULT}"
case "$COLOR_MODE" in
    always) c_on=1 ;;
    never)  c_on=0 ;;
    auto|*)
        if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then c_on=1; else c_on=0; fi
        ;;
esac

if [ "$c_on" = 1 ]; then
    LABEL=$(printf '\033%s' "$LABEL_ESC")
    BOLD=$( printf '\033%s' "$BOLD_ESC")
    OK=$(   printf '\033%s' "$OK_ESC")
    WARN=$( printf '\033%s' "$WARN_ESC")
    CRIT=$( printf '\033%s' "$CRIT_ESC")
    W=$(    printf '\033[0m')
else
    LABEL=""; BOLD=""; OK=""; WARN=""; CRIT=""; W=""
fi

# ── data collection ────────────────────────────────────────────────
CPU=""
if command -v top >/dev/null 2>&1; then
    CPU=$(top -bn1 2>/dev/null | awk '
        /[Cc]pu/ {
            for (i = 1; i <= NF; i++) {
                if ($i ~ /id/ && i > 1) {
                    v = $(i-1); gsub(/[^0-9.]/, "", v);
                    if (v != "") { printf "%d", 100 - v; exit }
                }
            }
        }')
fi

RAM_TOTAL=""; RAM_USED=""; RAM_PCT=""
if command -v free >/dev/null 2>&1; then
    RAM_TOTAL=$(free -m 2>/dev/null | awk '/^Mem:/ {print $2}')
    RAM_USED=$( free -m 2>/dev/null | awk '/^Mem:/ {print $3}')
    if [ -n "$RAM_TOTAL" ] && [ "$RAM_TOTAL" -gt 0 ] 2>/dev/null; then
        RAM_PCT=$(( RAM_USED * 100 / RAM_TOTAL ))
    fi
fi

DISK=""
if command -v df >/dev/null 2>&1; then
    DISK=$(df -h / 2>/dev/null | awk 'NR==2 {print $3"/"$2" ("$5")"}')
fi

IPS=""
if command -v ip >/dev/null 2>&1; then
    IPS=$(ip -4 addr show 2>/dev/null \
          | awk '/inet / && !/127.0.0.1/ {print $2}' \
          | cut -d/ -f1 | tr '\n' ' ' | sed 's/ *$//')
elif command -v ifconfig >/dev/null 2>&1; then
    IPS=$(ifconfig 2>/dev/null \
          | awk '/inet / && !/127.0.0.1/ {print $2}' \
          | tr '\n' ' ' | sed 's/ *$//')
elif command -v hostname >/dev/null 2>&1; then
    IPS=$(hostname -I 2>/dev/null | sed 's/ *$//')
fi

UPTIME=$(uptime -p 2>/dev/null | sed 's/^up //')
if [ -z "$UPTIME" ]; then
    UPTIME=$(uptime 2>/dev/null \
             | awk -F'up ' '{print $2}' \
             | awk -F',' '{print $1}' \
             | sed 's/^ *//;s/ *$//')
fi

LAST=""
if command -v lastlog >/dev/null 2>&1; then
    LOUT=$(lastlog -u "${USER:-root}" 2>/dev/null | sed -n '2p')
    if [ -n "$LOUT" ] && ! printf '%s' "$LOUT" | grep -q 'Never logged in'; then
        LAST=$(printf '%s' "$LOUT" | awk '{$1=""; sub(/^ +/, ""); print}')
    fi
fi
if [ -z "$LAST" ] && command -v last >/dev/null 2>&1; then
    LAST=$(last -n1 -R "${USER:-root}" 2>/dev/null \
           | head -n1 | awk '{$1=""; sub(/^ +/, ""); print}')
fi
[ -z "$LAST" ] && LAST="N/A"

# ── colour thresholds ─────────────────────────────────────────────
colour_for() {  # $1 = number (possibly empty), echo colour escape
    n=$1
    if [ -z "$n" ]; then printf '%s' "$W"; return; fi
    if [ "$n" -ge 80 ] 2>/dev/null; then printf '%s' "$CRIT"; return; fi
    if [ "$n" -ge 60 ] 2>/dev/null; then printf '%s' "$WARN"; return; fi
    printf '%s' "$OK"
}
CC=$(colour_for "${CPU:-}")
RC=$(colour_for "${RAM_PCT:-}")

# ── render ────────────────────────────────────────────────────────
printf '\n  %s%s%s  %s%s%s\n' \
    "$BOLD" "$(hostname 2>/dev/null || echo localhost)" "$W" \
    "$LABEL" "$(date '+%a %d %b %Y %H:%M %Z' 2>/dev/null)" "$W"
[ -n "$IPS" ]       && printf '  %sIP:%s      %s\n'   "$LABEL" "$W" "$IPS"
[ -n "$UPTIME" ]    && printf '  %sUptime:%s  %s\n'   "$LABEL" "$W" "$UPTIME"
[ -n "$LAST" ]      && printf '  %sLast:%s    %s\n'   "$LABEL" "$W" "$LAST"
[ -n "$CPU" ]       && printf '  %sCPU:%s     %s%s%%%s\n' \
                              "$LABEL" "$W" "$CC" "$CPU" "$W"
[ -n "$RAM_TOTAL" ] && printf '  %sRAM:%s     %s%s/%s MB (%s%%)%s\n' \
                              "$LABEL" "$W" "$RC" "$RAM_USED" "$RAM_TOTAL" "$RAM_PCT" "$W"
[ -n "$DISK" ]      && printf '  %sDisk:%s    %s\n'   "$LABEL" "$W" "$DISK"
printf '\n'
