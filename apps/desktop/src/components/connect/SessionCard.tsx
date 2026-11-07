/**
 * 连接状态块：三态 + 退出 + 联机日志。
 *
 * 这里承载了联机页最容易做错的几个判断：
 * - 「连过但房间里没人」必须是黄点，不能显示绿灯（那是最骗人的状态）；
 * - 房主退出 = 房间消失、好友立刻掉线，必须二次确认；
 * - 日志是排障材料，原样给全文 + 一键复制，不做任何过滤。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import {
  Box,
  Button,
  Dialog,
  DialogActions,
  DialogContent,
  DialogContentText,
  DialogTitle,
  Stack,
  TextField,
  Typography,
  alpha,
} from "@mui/material";
import { invoke } from "@tauri-apps/api/core";

import { formatLatency } from "../../lib/format";
import { toAppError } from "../../lib/types";
import { playEnterElements } from "../../motion/pageMotion";
import SettingGroup from "../settings/SettingGroup";
import { BigCopy } from "./shared";
import type { ConnectDevice, ConnectState } from "./types";

/** 设备身份 → 圆点颜色。只用状态色，不引新色板。 */
const DEVICE_DOT: Record<ConnectDevice["kind"], string> = {
  self: "success.main",
  host: "warning.main",
  player: "info.main",
  node: "text.disabled",
};

export function SessionCard({
  state,
  onExit,
}: {
  state: ConnectState;
  onExit: () => void;
}) {
  const failed = state.phase === "failed";
  const connected = state.phase === "connected";
  // 连接过但房间里没人了（对方走了）不能显示绿灯，那是最容易骗人的状态
  const dot = failed
    ? "error.main"
    : connected && state.online
      ? "success.main"
      : "warning.main";
  const isHost = state.role === "host";
  /**
   * 退出前要不要确认。
   *
   * 房主退出 = 整个房间消失，好友立刻掉线；这种后果不能靠一个按钮静默完成。
   * 一个人都没有时就别拦了 —— 那种「关个空房间还要点两次」的摩擦只会让人烦。
   */
  const [exitOpen, setExitOpen] = useState(false);
  const exitNeedsConfirm = isHost && state.players > 0;

  // 联机日志：排障材料。后端只给尾部 256 KB，前端原样展示 + 一键复制。
  const [logOpen, setLogOpen] = useState(false);
  const [logText, setLogText] = useState<string | null>(null);
  const [logError, setLogError] = useState<string | null>(null);
  const openLog = () => {
    setLogOpen(true);
    setLogText(null);
    setLogError(null);
    void invoke<string>("connect_log")
      .then((text) => setLogText(text))
      .catch((err: unknown) => setLogError(toAppError(err, "读取联机日志失败").message));
  };
  // 设备是轮询回来的（第一秒后才有人）。已经渲染的行不重播动画，
  // 只让新出现的那几行依次落下 —— 否则整列每轮都在跳。
  const deviceBoxRef = useRef<HTMLDivElement>(null);
  const shownDevices = useRef(0);
  useEffect(() => {
    const count = state.devices.length;
    const box = deviceBoxRef.current;
    if (box && count > shownDevices.current) {
      const added = Array.from(box.children)
        .slice(shownDevices.current)
        .filter((el): el is HTMLElement => el instanceof HTMLElement);
      playEnterElements(added);
    }
    shownDevices.current = count;
  }, [state.devices.length]);

  return (
    <>
      <SettingGroup
        title="当前房间"
        desc={
          isHost
            ? state.invite
              ? "把邀请码发给朋友，等他进来。"
              : "还没填游戏端口，填好才会生成邀请码。"
            : connected
              ? "已经连上房主，直接进游戏就行。"
              : "正在连到房主。"
        }
      >
        <Box sx={{ px: 2, py: 1.5 }}>
          <Stack direction="row" sx={{ gap: 1, alignItems: "center", flexWrap: "wrap" }}>
            <Box
              sx={{
                width: 8,
                height: 8,
                borderRadius: "50%",
                bgcolor: dot,
                // 未连接时呼吸，暗示还在进行中
                animation: failed || connected ? "none" : "xcl-pulse 1.6s ease-in-out infinite",
                "@keyframes xcl-pulse": { "0%,100%": { opacity: 1 }, "50%": { opacity: 0.35 } },
              }}
            />
            <Typography sx={{ fontSize: 13 }}>{state.label}</Typography>

            {connected && state.online && (
              <Typography sx={{ fontSize: 13, color: "text.secondary" }}>
                · {state.players + 1} 人
                {state.bestLatencyMs != null &&
                  ` · ${state.viaRelay ? "经中继" : "点对点"} · ${formatLatency(state.bestLatencyMs)}`}
              </Typography>
            )}

            {!connected && isHost && (
              <Typography sx={{ fontSize: 13, color: "text.secondary" }}>· 0 人</Typography>
            )}

            <Box sx={{ flex: 1 }} />

            <Button size="small" onClick={openLog}>
              日志
            </Button>

            <Button
              variant="outlined"
              size="small"
              onClick={() => {
                if (exitNeedsConfirm) {
                  setExitOpen(true);
                  return;
                }
                void invoke("connect_stop");
                onExit();
              }}
            >
              退出房间
            </Button>
          </Stack>

          {state.room && (
            <Typography sx={{ fontSize: 12, color: "text.secondary", mt: 0.75, userSelect: "all" }}>
              房间：{state.room}
            </Typography>
          )}

          {/* 房主的「游戏端口」：不在房间身份里，得单独摆出来，让他转告好友 */}
          {isHost && <HostPortRow initialPort={state.mcPort} />}

          {/* 房主唯一要做的事：把这条码发出去。整行实心按钮，别缩到角落里 */}
          {isHost && state.invite && (
            <BigCopy text={state.invite} label="复制邀请码" copiedLabel="已复制，发给好友吧" />
          )}

          {/* 加入方唯一要做的事：把进服地址填进游戏 */}
          {!isHost && state.localAddress && (
            <Box>
              <BigCopy
                text={state.localAddress}
                label="复制进服地址"
                copiedLabel="已复制，粘到游戏的「直接连接」里"
              />
              {/* 光给个复制按钮等于让人猜输在哪；隧道没通时进游戏还会失败，
                  看着像「联机坏了」，所以这两种状态要分开说。 */}
              <Typography sx={{ fontSize: 13, color: "text.secondary", mt: 0.75 }}>
                进服地址：
                <Box component="span" sx={{ userSelect: "all", fontWeight: 600 }}>
                  {state.localAddress}
                </Box>
              </Typography>
              <Typography sx={{ fontSize: 12.5, color: "text.secondary", mt: 0.5, lineHeight: 1.6 }}>
                {state.online
                  ? "在游戏「多人游戏 → 直接连接」里粘贴这个地址。"
                  : "隧道还在建立，等状态变成「已连接」再进游戏更稳。"}
              </Typography>
            </Box>
          )}

          {/* 房间里现在有谁：本机 / 房主 / 成员 / 服务器，顺序由后端排好 */}
          {state.devices.length > 0 && (
            <Box
              ref={deviceBoxRef}
              sx={{ mt: 1.5, pt: 1, borderTop: "1px solid", borderColor: "divider" }}
            >
              {state.devices.map((device) => (
                <Stack
                  key={`${device.kind}-${device.address ?? device.name}`}
                  direction="row"
                  sx={{ gap: 1, alignItems: "center", py: 0.4 }}
                >
                  <Box
                    sx={{
                      width: 6,
                      height: 6,
                      borderRadius: "50%",
                      flexShrink: 0,
                      bgcolor: DEVICE_DOT[device.kind],
                    }}
                  />
                  <Typography
                    sx={{
                      fontSize: 12.5,
                      flex: 1,
                      minWidth: 0,
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                      whiteSpace: "nowrap",
                    }}
                  >
                    {device.name}
                  </Typography>
                  <Typography sx={{ fontSize: 11.5, color: "text.disabled", flexShrink: 0 }}>
                    {device.label}
                  </Typography>
                  <Typography
                    sx={{
                      fontSize: 11.5,
                      color: "text.disabled",
                      flexShrink: 0,
                      fontVariantNumeric: "tabular-nums",
                    }}
                  >
                    {device.address ?? ""}
                  </Typography>
                  <Typography
                    sx={{
                      fontSize: 11.5,
                      color: "text.secondary",
                      flexShrink: 0,
                      minWidth: 56,
                      textAlign: "right",
                      fontVariantNumeric: "tabular-nums",
                    }}
                  >
                    {device.latencyMs == null
                      ? "—"
                      : `${formatLatency(device.latencyMs)}${device.viaRelay ? " 中继" : ""}`}
                  </Typography>
                </Stack>
              ))}
            </Box>
          )}

          {state.invite && (
            <Typography
              sx={{
                fontSize: 12,
                color: "text.secondary",
                mt: 1,
                wordBreak: "break-all",
                userSelect: "all",
              }}
            >
              邀请码：{state.invite}
            </Typography>
          )}

          {failed && state.error && (
            <Typography sx={{ fontSize: 12, color: "error.main", mt: 1 }}>{state.error}</Typography>
          )}
        </Box>
      </SettingGroup>

      {/* 房主退出 = 房间消失、好友掉线。这种后果不该被一个按钮静默带过 */}
      <Dialog open={exitOpen} onClose={() => setExitOpen(false)} maxWidth="xs" fullWidth>
        <DialogTitle sx={{ fontSize: 16, fontWeight: 700 }}>退出房间？</DialogTitle>
        <DialogContent>
          <DialogContentText sx={{ fontSize: 13 }}>
            你是房主，房间里还有 {state.players} 个人。你退出之后，进服地址指向的这台机器就没了，
            他们会被游戏断开。想再一起玩，得你重新开一次房，再把邀请码发给他们。
          </DialogContentText>
        </DialogContent>
        <DialogActions sx={{ px: 3, pb: 2 }}>
          <Button onClick={() => setExitOpen(false)}>取消</Button>
          <Button
            variant="contained"
            color="error"
            onClick={() => {
              setExitOpen(false);
              void invoke("connect_stop");
              onExit();
            }}
          >
            退出
          </Button>
        </DialogActions>
      </Dialog>

      {/* 联机日志：连不上时唯一能自证的材料，直接给全文 + 复制 */}
      <Dialog open={logOpen} onClose={() => setLogOpen(false)} maxWidth="md" fullWidth>
        <DialogTitle sx={{ fontSize: 16, fontWeight: 700, pb: 1 }}>联机日志</DialogTitle>
        <DialogContent sx={{ pb: 1 }}>
          {logError && (
            <DialogContentText sx={{ fontSize: 13, color: "error.main" }}>{logError}</DialogContentText>
          )}
          {!logError && logText === null && (
            <DialogContentText sx={{ fontSize: 13 }}>正在读取…</DialogContentText>
          )}
          {!logError && logText !== null && (
            <Box
              component="pre"
              sx={{
                m: 0,
                p: 1.5,
                height: "52vh",
                overflow: "auto",
                scrollbarGutter: "stable",
                fontSize: 11.5,
                lineHeight: 1.6,
                fontFamily: "ui-monospace, Consolas, monospace",
                bgcolor: (t) => alpha(t.palette.text.primary, 0.04),
                borderRadius: 1,
                whiteSpace: "pre-wrap",
                wordBreak: "break-all",
              }}
            >
              {logText.trim() || "（日志是空的）"}
            </Box>
          )}
        </DialogContent>
        <DialogActions sx={{ px: 3, pb: 2 }}>
          <Button onClick={() => setLogOpen(false)}>关闭</Button>
          <Button
            variant="contained"
            disabled={!logText}
            onClick={() => {
              if (logText) void navigator.clipboard?.writeText(logText);
              setLogOpen(false);
            }}
          >
            复制全部
          </Button>
        </DialogActions>
      </Dialog>
    </>
  );
}

/**
 * 房主的「游戏端口」。
 *
 * 端口写在邀请码里，所以这里只要一变，上面那条码就会实时更新 ——
 * 扫得到就自动填，扫不到就让房主自己填（游戏开在哪个口，只有他知道）。
 */
function HostPortRow({ initialPort }: { initialPort: number | null }) {
  const [port, setPort] = useState(() => (initialPort ? String(initialPort) : ""));
  const [scanning, setScanning] = useState(false);
  const [copied, setCopied] = useState(false);

  /*
   * 不自动扫。创建房间那一步已经扫过一次并把端口带进来了，
   * 每次进这个页面再扫一遍只会白跑 netstat、还可能把手填的值冲掉。
   * 只有点「扫描」才扫。
   */
  const scan = useCallback(async () => {
    setScanning(true);
    try {
      const ports = await invoke<number[]>("connect_scan_port");
      const found = ports[0];
      if (found != null) setPort(String(found));
    } catch {
      // 扫不到就算了，自己填
    } finally {
      setScanning(false);
    }
  }, []);

  // 端口一变就把新码推上来（防抖，避免每敲一个数字都跑一趟）
  useEffect(() => {
    const value = Number(port);
    if (!Number.isInteger(value) || value <= 0 || value > 65535) return;
    const timer = window.setTimeout(() => {
      void invoke("connect_set_port", { port: value }).catch(() => undefined);
    }, 500);
    return () => window.clearTimeout(timer);
  }, [port]);

  const valid = Number(port) > 0 && Number(port) <= 65535;

  return (
    <Box sx={{ mt: 1.5, pt: 1, borderTop: "1px solid", borderColor: "divider" }}>
      <Stack direction="row" sx={{ gap: 1, alignItems: "center", flexWrap: "wrap" }}>
        <TextField
          label="游戏端口"
          size="small"
          value={port}
          onChange={(event) => setPort(event.target.value.replace(/[^0-9]/g, ""))}
          placeholder="扫不到就自己填"
          sx={{ width: 180 }}
          error={port.length > 0 && !valid}
        />
        <Button variant="outlined" size="small" onClick={() => void scan()} disabled={scanning}>
          {scanning ? "扫描中…" : "扫描"}
        </Button>
        {valid && (
          <Button
            variant="text"
            size="small"
            onClick={() => {
              void navigator.clipboard
                ?.writeText(port)
                .then(() => {
                  setCopied(true);
                  window.setTimeout(() => setCopied(false), 1500);
                })
                .catch(() => {});
            }}
          >
            {copied ? "已复制" : "复制"}
          </Button>
        )}
      </Stack>
      <Typography sx={{ fontSize: 12.5, color: "text.secondary", mt: 0.75, lineHeight: 1.6 }}>
        {valid
          ? "这个端口已经写进上面的邀请码，好友粘一条就能进。"
          : "顺序不能反：先在游戏里按 Esc 点「对局域网开放」，再把端口填到这里。填好才会生成邀请码。"}
      </Typography>
    </Box>
  );
}
