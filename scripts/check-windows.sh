#!/usr/bin/env bash
# 在 macOS 上对 Windows 目标跑 clippy，提前发现 Windows 下的编译错误和死代码。
# 以 CI 的 windows-latest 为准；这里用 clang-cl + xwin 的 MSVC 头文件，只做提前拦截。
set -euo pipefail

target=x86_64-pc-windows-msvc

root=$(git rev-parse --show-toplevel)
cd "$root"

if [[ $(uname -s) != Darwin ]]; then
	echo "check-windows.sh 只在 macOS 上使用。" >&2
	exit 2
fi

if command -v brew >/dev/null 2>&1; then
	llvm_bin="$(brew --prefix llvm 2>/dev/null)/bin"
	if [[ -d $llvm_bin ]]; then
		export PATH="$llvm_bin:$PATH"
	fi
fi

missing=()
command -v cargo-xwin >/dev/null 2>&1 || missing+=("cargo install --locked cargo-xwin")
command -v clang-cl >/dev/null 2>&1 || missing+=("brew install llvm")
rustup target list --installed 2>/dev/null | grep -qx "$target" || missing+=("rustup target add $target")
if ((${#missing[@]})); then
	echo "缺少 Windows 交叉检查工具，请先执行：" >&2
	printf '  %s\n' "${missing[@]}" >&2
	exit 3
fi

CARGO_TARGET_DIR="$root/target/xwin" \
	cargo xwin clippy --workspace --all-targets --target "$target" -- -D warnings
