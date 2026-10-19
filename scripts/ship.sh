#!/usr/bin/env bash
# 提交、推送到远端，并触发安装包构建。agent 只需传入 Conventional Commits 信息。
set -euo pipefail

usage() {
	echo "用法：scripts/ship.sh <commit-message>" >&2
	echo "      scripts/ship.sh <<'EOF'" >&2
	echo "      feat: 标题" >&2
	echo "" >&2
	echo "      正文（可选）" >&2
	echo "      EOF" >&2
	exit 2
}

root=$(git rev-parse --show-toplevel)
cd "$root"

msg=""
if [[ $# -gt 0 ]]; then
	msg="$*"
elif [[ ! -t 0 ]]; then
	msg=$(cat)
fi
msg=${msg#"${msg%%[![:space:]]*}"}
msg=${msg%"${msg##*[![:space:]]}"}

branch=$(git rev-parse --abbrev-ref HEAD)
if [[ $branch == HEAD ]]; then
	echo "不能在游离 HEAD 上发布。" >&2
	exit 1
fi

dirty=0
if [[ -n $(git status --porcelain) ]]; then
	dirty=1
fi

if ((dirty)); then
	if [[ -z $msg ]]; then
		usage
	fi
	title=$(printf '%s\n' "$msg" | head -n 1)
	if ! printf '%s' "$title" | grep -Eq '^(feat|fix|docs|style|refactor|test|chore|perf)(\([^)]+\))?(!)?:[[:space:]]'; then
		echo "提交信息须为 Conventional Commits，例如：feat: 实现 Windows DDC 读写" >&2
		exit 1
	fi
	cargo fmt --all
	cargo clippy --workspace --all-targets -- -D warnings
	if [[ $(uname -s) == Darwin ]]; then
		rc=0
		scripts/check-windows.sh || rc=$?
		if ((rc == 3)); then
			echo "警告：未装 Windows 交叉检查工具，跳过本地 Windows clippy，以 CI 为准。" >&2
		elif ((rc != 0)); then
			exit "$rc"
		fi
	fi
	cargo test --workspace
	git add -A
	staged=$(git diff --cached --name-only)
	if [[ -z $staged ]]; then
		echo "没有可提交的改动。" >&2
		exit 1
	fi
	if echo "$staged" | grep -E '(^|/)\.env$|(^|/)credentials\.json$|\.pem$|(^|/)id_rsa$|(^|/)secrets\.toml$' >/dev/null; then
		echo "暂存区含可能的密钥文件，已中止：" >&2
		echo "$staged" >&2
		exit 1
	fi
	git commit -m "$msg"
fi

ahead=0
if git rev-parse --abbrev-ref '@{upstream}' >/dev/null 2>&1; then
	ahead=$(git rev-list --count '@{upstream}..HEAD')
else
	ahead=1
fi
if ((ahead == 0)); then
	echo "没有需要推送的提交。" >&2
	exit 1
fi

before=$(gh run list --workflow=pack.yml --branch "$branch" --limit 1 --json databaseId --jq '.[0].databaseId // empty' 2>/dev/null || true)
git push -u origin HEAD
gh workflow run pack.yml --ref "$branch"
for _ in 1 2 3 4 5 6 7 8; do
	sleep 2
	id=$(gh run list --workflow=pack.yml --branch "$branch" --limit 1 --json databaseId --jq '.[0].databaseId // empty')
	if [[ -n $id && $id != "$before" ]]; then
		gh run list --workflow=pack.yml --branch "$branch" --limit 1
		exit 0
	fi
done
echo "已 push，但未能确认 pack.yml 已排队。" >&2
exit 1
