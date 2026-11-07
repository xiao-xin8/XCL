#!/bin/sh
# 把账号服务装到目标机器上。
#
# 在开发机上跑：
#   ./deploy/install.sh <服务器地址>
#
# 前提：能用 ssh 免密登录，目标机器上已有 easytier core 在 127.0.0.1:11011 监听。
set -eu

HOST="${1:?用法: install.sh <user@host>}"
BIN=dist/xcld
SERVICE=deploy/xcld.service
RECOVER_SH=deploy/xcld-recover.sh
RECOVER_SVC=deploy/xcld-recover.service
RECOVER_TIMER=deploy/xcld-recover.timer

echo "==> 交叉编译（纯 Go，无 CGO）"
mkdir -p dist
CGO_ENABLED=0 GOOS=linux GOARCH=amd64 \
    go build -trimpath -ldflags "-s -w" -o "$BIN" ./cmd/xcld
ls -lh "$BIN"

echo "==> 上传"
# 先传到临时位置，避免覆盖正在运行的二进制（Linux 上其实可以，
# 但 scp 到被占用的文件会报 Text file busy）
scp "$BIN" "$HOST:/tmp/xcld.new"
scp "$SERVICE" "$HOST:/tmp/xcld.service.new"
scp "$RECOVER_SH" "$HOST:/tmp/xcld-recover.sh.new"
scp "$RECOVER_SVC" "$HOST:/tmp/xcld-recover.service.new"
scp "$RECOVER_TIMER" "$HOST:/tmp/xcld-recover.timer.new"

echo "==> 安装（需要 sudo）"
ssh "$HOST" 'sudo sh -s' <<'REMOTE'
set -eu

# 服务运行账号
id -u xcld >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin xcld

mkdir -p /opt/xcld /var/lib/xcld /etc/xcld
install -m 0755 /tmp/xcld.new /opt/xcld/xcld
chown -R xcld:xcld /var/lib/xcld
chmod 0750 /var/lib/xcld

# 密钥文件只在首次安装时生成，重装不会覆盖已有的
if [ ! -f /etc/xcld/xcld.env ]; then
    echo "==> 生成密钥"
    /opt/xcld/xcld keygen > /etc/xcld/xcld.env
    echo "# 可选：不填则使用默认验证方式" >> /etc/xcld/xcld.env
    echo "XCL_TURNSTILE_SECRET=" >> /etc/xcld/xcld.env
    echo "XCL_TURNSTILE_SITEKEY=" >> /etc/xcld/xcld.env
    chmod 0640 /etc/xcld/xcld.env
    chown root:xcld /etc/xcld/xcld.env
    echo "！！ 公钥（要写进客户端）："
    grep XCL_SIGN_PUBKEY /etc/xcld/xcld.env
else
    echo "==> 已存在 /etc/xcld/xcld.env，保留原密钥"
fi

install -m 0644 /tmp/xcld.service.new /etc/systemd/system/xcld.service
install -m 0755 /tmp/xcld-recover.sh.new /opt/xcld/recover.sh
install -m 0644 /tmp/xcld-recover.service.new /etc/systemd/system/xcld-recover.service
install -m 0644 /tmp/xcld-recover.timer.new /etc/systemd/system/xcld-recover.timer

# 看门狗要能 systemctl start，所以以 root 跑（oneshot，只在冷却结束时动一次）
systemctl daemon-reload
systemctl enable xcld
systemctl enable --now xcld-recover.timer
systemctl restart xcld
sleep 1
systemctl --no-pager --lines=15 status xcld || true

rm -f /tmp/xcld.new /tmp/xcld.service.new \
      /tmp/xcld-recover.sh.new /tmp/xcld-recover.service.new /tmp/xcld-recover.timer.new
REMOTE

echo
echo "==> 自检"
ssh "$HOST" 'curl -fsS http://127.0.0.1:8080/v1/health' || echo "健康检查失败，看 journalctl -u xcld"
echo
echo "提示：内核层限速没自动装（可能和你已有的防火墙冲突），需要的话手动执行："
echo "    scp deploy/nftables-xcl.conf $HOST:/tmp/ && ssh $HOST sudo nft -f /tmp/nftables-xcl.conf"
