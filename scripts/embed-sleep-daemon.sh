#!/usr/bin/env bash
# 把 SMAppService 用的 LaunchDaemon plist 放进 .app 的 Contents/Library/LaunchDaemons/。
set -euo pipefail

if [[ "$(uname -s)" != Darwin ]]; then
	exit 0
fi

root=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
plist="$root/crates/baton-gui/macos/LaunchDaemons/io.github.rossettaylm.deskbaton.sleep.plist"
app=""
for candidate in \
	"$root/target/release/bundle/macos/DeskBaton.app" \
	"$root/crates/baton-gui/target/release/bundle/macos/DeskBaton.app"
do
	if [[ -d $candidate ]]; then
		app=$candidate
		break
	fi
done
if [[ -z $app ]]; then
	app=$(find "$root/target" "$root/crates/baton-gui/target" -name 'DeskBaton.app' -type d 2>/dev/null | head -n 1 || true)
fi
if [[ -z $app || ! -f $plist ]]; then
	echo "找不到 DeskBaton.app 或 sleep daemon plist，跳过嵌入。" >&2
	exit 0
fi
dest="$app/Contents/Library/LaunchDaemons"
mkdir -p "$dest"
cp "$plist" "$dest/io.github.rossettaylm.deskbaton.sleep.plist"
echo "已写入 $dest/io.github.rossettaylm.deskbaton.sleep.plist"
