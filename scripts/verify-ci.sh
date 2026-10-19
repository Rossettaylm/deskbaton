#!/usr/bin/env bash
# 把当前工作区（含未提交改动）打成快照，推到 agent/verify-<分支>，等 CI 结果。
# 不改本地分支、暂存区和工作区；快照分支每次强推，只用于验证，不合入。
# --pack：再触发 pack.yml，等安装包构建和 Windows 安装冒烟测试。
set -euo pipefail

usage() {
	echo "用法：scripts/verify-ci.sh [--pack]" >&2
	exit 2
}

pack=0
case "${1:-}" in
"") ;;
--pack) pack=1 ;;
*) usage ;;
esac

root=$(git rev-parse --show-toplevel)
cd "$root"

branch=$(git rev-parse --abbrev-ref HEAD)
if [[ $branch == HEAD ]]; then
	echo "不能在游离 HEAD 上验证。" >&2
	exit 1
fi
ref="agent/verify-${branch//\//-}"

tmp_index=$(mktemp)
cp "$(git rev-parse --git-path index)" "$tmp_index"
GIT_INDEX_FILE=$tmp_index git add -A
tree=$(GIT_INDEX_FILE=$tmp_index git write-tree)
rm -f "$tmp_index"

changed=$(git diff --name-only HEAD "$tree")
if echo "$changed" | grep -E '(^|/)\.env$|(^|/)credentials\.json$|\.pem$|(^|/)id_rsa$|(^|/)secrets\.toml$' >/dev/null; then
	echo "快照含可能的密钥文件，已中止：" >&2
	echo "$changed" >&2
	exit 1
fi

if [[ -z $changed ]]; then
	commit=$(git rev-parse HEAD)
else
	commit=$(git commit-tree "$tree" -p HEAD -m "chore: agent 验证快照（不合入）")
fi

git push --force origin "$commit:refs/heads/$ref"
echo "已推送 $commit 到 $ref"

find_run() {
	local workflow=$1 event=$2 id
	for _ in $(seq 1 30); do
		id=$(gh run list --workflow "$workflow" --branch "$ref" --event "$event" --limit 20 \
			--json databaseId,headSha --jq ".[] | select(.headSha == \"$commit\") | .databaseId" | head -n 1)
		if [[ -n $id ]]; then
			echo "$id"
			return 0
		fi
		sleep 2
	done
	echo "未找到 $workflow 在 $ref 上对应 $commit 的运行。" >&2
	return 1
}

watch_run() {
	local name=$1 id=$2
	echo "等待 ${name}：$(gh run view "$id" --json url --jq .url)"
	if gh run watch "$id" --exit-status --compact --interval 15 >/dev/null; then
		echo "${name} 通过"
		return 0
	fi
	echo "${name} 失败，失败步骤日志：" >&2
	gh run view "$id" --log-failed | tail -n 200 >&2
	return 1
}

ci_id=$(find_run ci.yml push)
if ((pack)); then
	gh workflow run pack.yml --ref "$ref"
	pack_id=$(find_run pack.yml workflow_dispatch)
fi

status=0
watch_run CI "$ci_id" || status=1
if ((pack)); then
	watch_run Pack "$pack_id" || status=1
fi
exit "$status"
