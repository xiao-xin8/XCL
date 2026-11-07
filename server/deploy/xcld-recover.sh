#!/bin/sh
# 看门狗：xcld 因为熔断主动退出后，按冷却时间把它拉回来。
#
# 为什么不用 systemd 的 Restart=always：被攻击时立刻重启等于白给 ——
# 刚起来就被再打一次，CPU 全花在冷启动上。所以主动退出（exit 75）不重启，
# 由这个脚本在冷却时间到了之后再 start。
#
# 判定依据只有一条：/var/lib/xcld/cooling.json 是否存在、resumeAt 是否已过。
# 没有这个文件就完全不干预 —— 人工 systemctl stop 的场合必须尊重。
set -eu

COOL=/var/lib/xcld/cooling.json

[ -f "$COOL" ] || exit 0

NOW=$(date +%s)
RESUME=$(sed -n 's/.*"resumeAt"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$COOL" | head -1)

# 文件坏了：删掉重来，别让一个坏文件把服务永久钉死
if [ -z "$RESUME" ]; then
    echo "cooling.json 读不出 resumeAt，已删除"
    rm -f "$COOL"
    systemctl start xcld
    exit 0
fi

if [ "$NOW" -lt "$RESUME" ]; then
    exit 0
fi

echo "冷却结束，重新拉起 xcld"
systemctl start xcld
