#!/bin/bash
# 实测：合盖睡眠的 Mac 能否被 Wi-Fi 魔术包唤醒，醒来后能否不开盖接管画面和键鼠。
# Windows 端脚本由本脚本经临时 HTTP 服务下发，只服务一次请求，每次实测都要重新运行本脚本。
# mDNSResponder 只在本机宣告了 Bonjour 服务时才在睡前做网络唤醒相关配置（mDNSCore/mDNS.c BeginSleepProcessing），
# 魔术包唤醒是否依赖它待对照，所以默认在睡眠期间宣告一个 _deskbaton._tcp 服务；-n 不宣告，用作对照。
# 用法：scripts/probe/mac-wol-wake.sh [-w Windows 发包前等待秒数，默认 60] [-i 网卡，默认 en0] [-n]
set -u

IFACE=en0
WAIT_SEC=60
PORT=8765
OBSERVE_SEC=180
KEYBOARD_PATTERN='keyboard|receiver'
ADVERTISE=1
PMSET=/usr/bin/pmset

while getopts "w:i:n" opt; do
	case $opt in
	w) WAIT_SEC=$OPTARG ;;
	i) IFACE=$OPTARG ;;
	n) ADVERTISE=0 ;;
	*)
		echo "用法：$0 [-w 等待秒数] [-i 网卡] [-n 不宣告 Bonjour 服务]"
		exit 2
		;;
	esac
done

HERE=$(cd "$(dirname "$0")" && pwd)
LOG_DIR="$HOME/Library/Logs/DeskBaton"
mkdir -p "$LOG_DIR"
LOG="$LOG_DIR/wol-probe-$(date +%Y%m%d-%H%M%S).log"

log() { echo "$(date '+%H:%M:%S') $*" | tee -a "$LOG"; }
die() {
	log "错误：$*"
	exit 1
}

caffeinate_pid=
advertise_pid=
served=
sleep_disabled_by_us=0
cleanup() {
	[ -n "$caffeinate_pid" ] && kill "$caffeinate_pid" 2>/dev/null
	[ -n "$advertise_pid" ] && kill "$advertise_pid" 2>/dev/null
	[ -n "$served" ] && rm -f "$served"
	if [ "$sleep_disabled_by_us" = 1 ] && sudo -n $PMSET -a disablesleep 0; then
		log "已恢复 SleepDisabled=0"
	fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM

pmset_value() { $PMSET -g | awk -v k="$1" '$1 == k { print $2; exit }'; }
usb_keyboard_present() { ioreg -p IOUSB -w0 | grep -Eqi "$KEYBOARD_PATTERN"; }

# 打印两次相邻检查之间超过 5 秒的停顿（即系统睡过）；给了上限且超时则返回 1。
wait_for_gap() {
	local limit=$1 start last now
	start=$(date +%s)
	last=$start
	while :; do
		sleep 1
		now=$(date +%s)
		if ((now - last > 5)); then
			echo $((now - last))
			return 0
		fi
		last=$now
		if [ -n "$limit" ] && ((now - start >= limit)); then
			return 1
		fi
	done
}

# ---------- 前提检查 ----------
case $WAIT_SEC in
'' | *[!0-9]*) die "-w 要求整数秒" ;;
esac
((WAIT_SEC >= 20)) || die "-w 至少 20 秒，Mac 要先睡着"

mac=$(ifconfig "$IFACE" 2>/dev/null | awk '/ether/ { print $2; exit }')
[ -n "$mac" ] || die "找不到网卡 $IFACE"
hw_mac=$(networksetup -getmacaddress "$IFACE" 2>/dev/null | awk '{ print $3 }')
ip=$(ipconfig getifaddr "$IFACE") || die "$IFACE 没有 IPv4 地址，先连上 Wi-Fi"
broadcast=$(ifconfig "$IFACE" | awk '/inet / { for (i = 1; i < NF; i++) if ($i == "broadcast") print $(i + 1) }')

[ "$(pmset_value womp)" = 1 ] || die "「唤醒以供网络访问」没开（pmset womp 不是 1）：系统设置 → 电池 → 选项"

if [ "$(pmset_value SleepDisabled)" = 1 ]; then
	sudo -n $PMSET -a disablesleep 0 2>/dev/null ||
		die "SleepDisabled=1 会挡住睡眠：先结束 stay-awake，或执行 sudo pmset -a disablesleep 0"
	log "已把残留的 SleepDisabled 置回 0"
fi

if sudo -n -l $PMSET -a disablesleep 1 >/dev/null 2>&1; then
	has_rule=1
else
	has_rule=0
	log "警告：没装 sudoers 规则（deskbaton-cli sleep-helper install），醒来后切回 USB-C 时可能触发合盖睡眠"
fi

if usb_keyboard_present; then
	watch_keyboard=1
else
	watch_keyboard=0
	log "提示：USB 树里没找到匹配「${KEYBOARD_PATTERN}」的设备，不检测键鼠回归"
fi

log "网卡 ${IFACE}：当前 MAC ${mac}，IP ${ip}，广播 ${broadcast:-无}"
if [ -n "$hw_mac" ] && [ "$hw_mac" != "$mac" ]; then
	log "提示：在用私有 Wi-Fi 地址（硬件地址 ${hw_mac}），魔术包发给当前地址 ${mac}；若该网络设为「轮换」，地址会变"
fi
log "日志：$LOG"

# ---------- 下发 Windows 脚本 ----------
served=$(mktemp -t deskbaton-wol)
{
	printf "\$Mac = '%s'\n\$Ip = '%s'\n\$Broadcast = '%s'\n\$WaitSec = %d\n" "$mac" "$ip" "$broadcast" "$WAIT_SEC"
	cat "$HERE/win-send-wol.ps1"
} >"$served"

echo
log "1. 若弹出防火墙询问 python3 是否接受连接，点「允许」"
log "2. 用 OSD 切到 DP"
log "3. 在 Windows PowerShell 输入：irm http://$ip:$PORT | iex"
log "4. 等 Windows 提示后，用 OSD 切回 USB-C"
echo

/usr/bin/python3 - "$served" "$PORT" <<'PY' || die "临时 HTTP 服务失败（端口 $PORT 被占用？）"
import http.server, sys

body = open(sys.argv[1], "rb").read()

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass

http.server.HTTPServer(("0.0.0.0", int(sys.argv[2])), Handler).handle_request()
PY

log "Windows 已取走脚本，10 秒后让 Mac 睡眠"
if [ "$ADVERTISE" = 1 ]; then
	dns-sd -R "DeskBaton probe" _deskbaton._tcp local. 24800 >/dev/null 2>&1 &
	advertise_pid=$!
	log "已宣告 Bonjour 服务 _deskbaton._tcp"
else
	log "按 -n 不宣告 Bonjour 服务（对照组）"
fi
sleep 10

# ---------- 睡眠，等唤醒 ----------
sleep_at=$(date +%s)
sleep_since=$(date -r "$sleep_at" '+%Y-%m-%d %H:%M:%S')
log "pmset sleepnow"
$PMSET sleepnow >/dev/null

if ! gap=$(wait_for_gap 30); then
	log "警告：30 秒内没睡着，阻止睡眠的断言："
	$PMSET -g assertions | grep -E 'PreventSystemSleep|PreventUserIdleSystemSleep|NoIdleSleep' | tee -a "$LOG"
	gap=$(wait_for_gap "")
fi

# 系统约每 15 分钟会因 rtc 定时任务暗唤醒一次；只把 Wi-Fi（wifibt）和开盖（lid）当作被测的唤醒，其余让它自己睡回去。
ignored=0
while :; do
	wake_reason=$(sysctl -n kern.wakereason 2>/dev/null)
	case $wake_reason in
	*wifibt* | *lid*) break ;;
	esac
	ignored=$((ignored + 1))
	log "忽略非目标唤醒：${wake_reason:-未知}（这段睡了约 ${gap} 秒），等它睡回去"
	gap=$(wait_for_gap "")
done

woke_at=$(date +%s)
log "检测到醒来：${wake_reason}（入睡后约 $((woke_at - sleep_at)) 秒）。模拟 agent：声明用户活动点亮显示"
caffeinate -u -t "$OBSERVE_SEC" &
caffeinate_pid=$!
if [ "$has_rule" = 1 ] && sudo -n $PMSET -a disablesleep 1; then
	sleep_disabled_by_us=1
	log "已置 SleepDisabled=1，防止切回时合盖睡眠"
fi
log "现在可以用 OSD 切回 USB-C"

# ---------- 观察：键鼠回归、是否又睡 ----------
keyboard_back=
resleeps=0
end=$((woke_at + OBSERVE_SEC))
last=$woke_at
while (($(date +%s) < end)); do
	sleep 1
	now=$(date +%s)
	if ((now - last > 5)); then
		resleeps=$((resleeps + 1))
		log "又睡了约 $((now - last)) 秒"
	fi
	last=$now
	if [ "$watch_keyboard" = 1 ] && [ -z "$keyboard_back" ] && usb_keyboard_present; then
		keyboard_back=$now
		log "键鼠回到 Mac（醒来后 $((now - woke_at)) 秒），再观察 20 秒"
		end=$((now + 20))
	fi
done

# ---------- 汇总 ----------
events=$($PMSET -g log | awk -v s="$sleep_since" 'substr($0, 1, 19) >= s' |
	grep -E '^[0-9-]+ [0-9:]+ [+-][0-9]+ (Sleep|Wake|DarkWake|WakeDetails)[[:space:]]|Display is turned' |
	grep -v 'Wake Requests' | cut -c1-220)

# airportd 睡前的「WoW is not active」不代表收不到魔术包，以唤醒后它记录的唤醒来源为准。
wifi_wake=$(/usr/bin/log show --style compact --start "$(date -r $((woke_at - 5)) '+%Y-%m-%d %H:%M:%S')" \
	--end "$(date -r $((woke_at + 10)) '+%Y-%m-%d %H:%M:%S')" --predicate 'process == "airportd"' 2>/dev/null |
	grep -m1 "System wake reason:" | cut -c1-220)
case $wifi_wake in
*"was woken by WiFi"*) wifi_verdict="是" ;;
'') wifi_verdict="日志里没找到 airportd 的唤醒来源记录" ;;
*) wifi_verdict="否" ;;
esac

case $wake_reason in
*lid*) verdict="开盖唤醒：魔术包没能唤醒 Mac" ;;
*) verdict="Wi-Fi / 蓝牙唤醒于 $(date -r "$woke_at" '+%H:%M:%S')；与 Windows「已发魔术包」的时间相差几秒内才算魔术包，否则可能是推送等其他流量" ;;
esac

echo
log "==== pmset 日志：$sleep_since 之后的睡眠 / 唤醒事件 ===="
echo "$events" | tee -a "$LOG"
log "==== airportd：唤醒来源 ===="
echo "${wifi_wake:-无}" | tee -a "$LOG"
echo
log "==== 结论 ===="
log "宣告 Bonjour 服务：$([ "$ADVERTISE" = 1 ] && echo 是 || echo 否)"
log "入睡后约 $((woke_at - sleep_at)) 秒被唤醒，期间忽略了 ${ignored} 次非目标唤醒"
log "唤醒原因：$wake_reason"
log "被 Wi-Fi 唤醒：$wifi_verdict"
log "判断：$verdict"
if [ -n "$keyboard_back" ]; then
	log "键鼠：醒来后 $((keyboard_back - woke_at)) 秒回到 Mac"
elif [ "$watch_keyboard" = 1 ]; then
	log "键鼠：$OBSERVE_SEC 秒内没回到 Mac"
fi
log "醒来后又睡：$resleeps 次"
log "完整日志：${LOG}（把这段结论发给我）"
