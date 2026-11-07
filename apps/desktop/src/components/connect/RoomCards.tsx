/**
 * 创建 / 加入房间两张表单卡。
 *
 * 两者形状一致（步骤块 + 输入 + 整行实心按钮），差别只在字段和调用的命令，
 * 所以放一个文件；不要再抽「通用表单」—— 它们的分支只会越抽越多。
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
} from "@mui/material";
import { invoke } from "@tauri-apps/api/core";

import { BUTTON_SX } from "../../lib/buttons";
import { toAppError } from "../../lib/types";
import SettingGroup from "../settings/SettingGroup";
import { Steps } from "./shared";

/**
 * 创建房间（放最上面：房主是最常见的入口）。
 *
 * 不问端口：房主只负责把大家放进同一个虚拟网，游戏端口是「开房之后告诉好友」
 * 的运行时信息 —— 它每次开游戏都可能变，放进来房间身份就被绑死在一次会话上了。
 */
export function CreateRoomCard(props: {
  savedRoom: string;
  savedSecret: string;
  /** 已经看过「离线账号进不去」提示了吗（只弹一次） */
  hintSeen: boolean;
  onHintSeen: () => void;
}) {
  const { savedRoom, savedSecret, hintSeen, onHintSeen } = props;
  const [room, setRoom] = useState("");
  const [secret, setSecret] = useState("");
  const [hint, setHint] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // 第一次开房先解释「离线账号进不去」，看完再真开
  const [offlineOpen, setOfflineOpen] = useState(false);
  const seeded = useRef(false);

  // 配置读回来之后填一次（上次开房用的房间名 / 密码），之后以输入为准
  useEffect(() => {
    if (seeded.current || (!savedRoom && !savedSecret)) return;
    seeded.current = true;
    setRoom(savedRoom);
    setSecret(savedSecret);
  }, [savedRoom, savedSecret]);

  const start = useCallback(async () => {
    setBusy(true);
    setHint(null);
    try {
      // 端口只是顺手带上：扫到就用，扫不到就让房间卡里手动填，填完码会实时更新
      const ports = await invoke<number[]>("connect_scan_port").catch(() => [] as number[]);
      await invoke("connect_start_host", {
        room: room.trim(),
        secret: secret.trim(),
        port: ports[0] ?? 0,
      });
    } catch (err) {
      setHint(toAppError(err, "创建房间失败").message);
    } finally {
      setBusy(false);
    }
  }, [room, secret]);

  const canStart = room.trim().length > 0 && secret.trim().length > 0;

  /** 第一次开房先讲清「离线账号进不去」，这一步不拦人，只是把话说在前面。 */
  const begin = () => {
    if (!hintSeen) {
      setOfflineOpen(true);
      return;
    }
    void start();
  };

  const confirmOffline = () => {
    setOfflineOpen(false);
    onHintSeen();
    void start();
  };

  return (
    <>
      <SettingGroup
        title="创建房间"
        desc="房间名和密码都要填，会记下来，下次开房自动带出。"
      >
        <Box sx={{ px: 2, py: 1.5 }}>
          <Steps
            items={[
              "填好房间名和密码",
              "进游戏，按 Esc 点「对局域网开放」",
              "端口自动填好后，复制邀请码发给好友",
            ]}
          />
          <Stack sx={{ gap: 1 }}>
            <TextField
              label="房间名"
              size="small"
              required
              value={room}
              onChange={(event) => setRoom(event.target.value)}
              placeholder="例如 小鑫的房间"
              helperText="最多 24 个字符"
              error={room.length > 24}
            />
            <TextField
              label="房间密码"
              size="small"
              required
              value={secret}
              onChange={(event) => setSecret(event.target.value)}
              placeholder="好友靠它进同一个房间"
              helperText="改成别的就等于换了个新房间"
            />
            <Box>
              <Button
                fullWidth
                variant="contained"
                sx={BUTTON_SX}
                disabled={busy || !canStart || room.length > 24}
                onClick={begin}
              >
                创建房间
              </Button>
            </Box>
            {hint && <Typography sx={{ fontSize: 12, color: "error.main" }}>{hint}</Typography>}
          </Stack>
        </Box>
      </SettingGroup>

      {/* 只弹一次：离线账号的坑，每个第一次联机的人都会踩 */}
      <Dialog
        open={offlineOpen}
        onClose={() => setOfflineOpen(false)}
        maxWidth="xs"
        fullWidth
      >
        <DialogTitle sx={{ fontSize: 16, fontWeight: 700 }}>进游戏前先看一眼</DialogTitle>
        <DialogContent>
          <DialogContentText sx={{ fontSize: 13 }}>
            单人世界点「对局域网开放」时，在线验证默认是开着的。你和好友如果是离线账号，
            客户端会报「无效的会话」—— 那说明隧道是通的，只是账号没过验证。
          </DialogContentText>
          <DialogContentText sx={{ fontSize: 13, mt: 1 }}>
            想真的进到世界里，两条路选一条：房主装 OfflineLAN 并在开世界时关掉「在线模式」
            （加入方不用装）；或者双方都用第三方登录进游戏。
          </DialogContentText>
          <DialogContentText sx={{ fontSize: 12.5, color: "text.secondary", mt: 1 }}>
            双方都是正版账号的话，这条可以忽略。
          </DialogContentText>
        </DialogContent>
        <DialogActions sx={{ px: 3, pb: 2 }}>
          <Button onClick={() => setOfflineOpen(false)}>取消</Button>
          <Button variant="contained" onClick={confirmOffline}>
            知道了
          </Button>
        </DialogActions>
      </Dialog>
    </>
  );
}

/**
 * 加入房间。
 *
 * 一条码粘完就进 —— 端口在码里，不用再问房主。
 */
export function JoinRoomCard() {
  const [code, setCode] = useState(() => localStorage.getItem("xcl.connect.code") ?? "");
  const [hint, setHint] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (code.trim()) localStorage.setItem("xcl.connect.code", code.trim());
  }, [code]);

  const join = useCallback(async () => {
    setBusy(true);
    setHint(null);
    try {
      await invoke("connect_start_guest", { code: code.trim() });
    } catch (err) {
      setHint(toAppError(err, "加入房间失败").message);
    } finally {
      setBusy(false);
    }
  }, [code]);

  return (
    <SettingGroup title="加入房间" desc="把房主发你的邀请码整条粘进来就行，其它都不用填。">
      <Box sx={{ px: 2, py: 1.5 }}>
        <Steps
          items={[
            "把好友发你的邀请码整条粘进来",
            "点「加入房间」，等状态变成「已连接」",
            "进游戏 → 多人游戏 → 直接连接，粘下面的地址",
          ]}
        />
        <Stack sx={{ gap: 1 }}>
          <TextField
            label="邀请码"
            size="small"
            fullWidth
            multiline
            minRows={1}
            maxRows={3}
            value={code}
            onChange={(event) => setCode(event.target.value)}
            placeholder="XCL..."
          />
          <Box>
            <Button
              fullWidth
              variant="contained"
              sx={BUTTON_SX}
              disabled={busy || !code.trim()}
              onClick={() => void join()}
            >
              加入房间
            </Button>
          </Box>
          {hint && <Typography sx={{ fontSize: 12, color: "error.main" }}>{hint}</Typography>}
        </Stack>
      </Box>
    </SettingGroup>
  );
}
