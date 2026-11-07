import { type KeyboardEvent, useCallback, useEffect, useState } from "react";
import {
  Box,
  Button,
  Checkbox,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  Link,
  Stack,
  Tab,
  Tabs,
  TextField,
  Typography,
  alpha,
} from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import { toAppError } from "../lib/types";
import { BUTTON_SX } from "../lib/buttons";
import Reveal from "./settings/Reveal";

/**
 * 联机账号的登录状态（后端 `connect_account::AccountView`）。
 *
 * 这是我们自己的账号，不是 Minecraft 账号 —— 用来进节点，和 `auth` 那套
 * （离线 / 第三方登录、进游戏）完全是两件事。
 */
export type ConnectAccount = {
  signedIn: boolean;
  username: string | null;
  userId: number | null;
  /** 令牌过期时间（Unix 秒）。0 = 未登录。 */
  expiresAt: number;
  /** 当前连的账号服务器，出错时用来确认「我到底连的是哪个」。 */
  server: string;
};

/** 打开弹窗时落在哪一档。 */
export type ConnectAuthMode = "login" | "register";

/** 胶囊 Tab 的统一高度：外层 36（含 3px 内衬），里面按钮 30。 */
const PILL_H = 30;

/**
 * 联机账号登录 / 注册弹窗。
 *
 * 可随地引用：不依赖连接页的任何状态，只要给 `open` 和回调就能用。
 * 页面负责「没登录时摆什么」，这里只负责「怎么登录」。
 *
 * 登录 / 注册靠顶部胶囊 Tab 切换（样式对齐 Mod 列表页的源切换），
 * 不做双栏、不做底部切换链接 —— 一张卡、一排字段、一个按钮，就这么简单。
 */
export default function ConnectLoginDialog({
  open,
  initialMode = "login",
  onClose,
  onSignedIn,
}: {
  open: boolean;
  /** 每次打开时落在哪一档；打开后由弹窗自己管。 */
  initialMode?: ConnectAuthMode;
  onClose: () => void;
  onSignedIn: (account: ConnectAccount) => void;
}) {
  const [mode, setMode] = useState<ConnectAuthMode>(initialMode);
  const [username, setUsername] = useState(
    () => localStorage.getItem("xcl.connect.username") ?? "",
  );
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [agreed, setAgreed] = useState(false);
  const [termsOpen, setTermsOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [hint, setHint] = useState<string | null>(null);
  const registering = mode === "register";
  const mismatch = registering && confirm.length > 0 && confirm !== password;
  // 注册要多两道门：两次密码一致、须知勾上。少一道都能让人事后找上门。
  const canSubmit =
    username.trim().length > 0 &&
    password.length > 0 &&
    !busy &&
    (!registering || (confirm === password && agreed));

  /*
   * 每次打开都从干净状态开始：密码、确认密码、勾选、错误提示全部清掉，
   * 档位回到调用方要的那一档。用户名留着 —— 下次打开少敲一次。
   */
  useEffect(() => {
    if (!open) return;
    setMode(initialMode);
    setPassword("");
    setConfirm("");
    setAgreed(false);
    setHint(null);
  }, [open, initialMode]);

  const submit = useCallback(async () => {
    setBusy(true);
    setHint(null);
    try {
      const view = await invoke<ConnectAccount>(
        registering ? "connect_account_register" : "connect_account_login",
        { username: username.trim(), password },
      );
      // 只记用户名，绝不记密码：下次打开省一次输入，但密码永远由用户自己敲。
      localStorage.setItem("xcl.connect.username", username.trim());
      // 密码清掉再交出去，别让弹窗关闭期间还挂在内存里
      setPassword("");
      setConfirm("");
      onSignedIn(view);
    } catch (err) {
      setHint(toAppError(err, registering ? "注册失败" : "登录失败").message);
    } finally {
      setBusy(false);
    }
  }, [onSignedIn, password, registering, username]);

  const switchMode = (_event: unknown, next: ConnectAuthMode) => {
    setMode(next);
    setHint(null);
    // 确认密码和勾选只对注册有意义，切走就清掉，免得切回来是个「已勾但没读」的状态
    setConfirm("");
    setAgreed(false);
  };

  const onEnter = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Enter" && canSubmit) void submit();
  };

  return (
    <>
      <Dialog open={open} onClose={busy ? undefined : onClose} maxWidth="xs" fullWidth>
        <DialogTitle sx={{ fontSize: 16, fontWeight: 700, pb: 1 }}>XCL 账号</DialogTitle>
        <DialogContent sx={{ pt: "12px !important" }}>
          {/* 登录 / 注册：胶囊 Tab。外层一个大容器，里面的高亮块跟着选中项左右滑 */}
          <Box
            sx={{
              width: "100%",
              height: PILL_H + 6,
              p: "3px",
              borderRadius: 999,
              // 胶囊条也是「面」：跟着界面不透明度一起淡，开了模糊一起毛玻璃
              bgcolor: (t) =>
                alpha(t.palette.text.primary, 0.06 * t.palette.surface.opacity),
              backdropFilter: (t) => t.palette.surface.filter,
              display: "flex",
              alignItems: "center",
            }}
          >
            <Tabs
              value={mode}
              onChange={switchMode}
              variant="standard"
              sx={{
                width: "100%",
                minHeight: PILL_H,
                "& .MuiTabs-flexContainer": { gap: 0 },
                "& .MuiTabs-indicator": {
                  height: PILL_H,
                  top: 0,
                  borderRadius: 999,
                  bgcolor: "primary.main",
                  // 淡黄和背景太近，靠一圈深金细边分出来
                  boxShadow: (t) =>
                    `inset 0 0 0 1px ${alpha(t.palette.brand.ink, 0.22)}`,
                },
                "& .MuiTab-root": {
                  flex: 1,
                  minHeight: PILL_H,
                  height: PILL_H,
                  minWidth: 0,
                  px: 2,
                  zIndex: 1,
                  borderRadius: 999,
                  fontSize: 13,
                  fontWeight: 500,
                  textTransform: "none",
                  whiteSpace: "nowrap",
                  color: "text.secondary",
                  transition: "color 200ms ease",
                  "&.Mui-selected": { color: "primary.contrastText" },
                },
                "& .MuiTab-root:not(.Mui-selected):hover": {
                  color: "text.primary",
                  bgcolor: (t) => alpha(t.palette.text.primary, 0.06),
                },
              }}
            >
              <Tab value="login" label="登录" disableRipple />
              <Tab value="register" label="注册" disableRipple />
            </Tabs>
          </Box>

          {/* 一句话说清这是什么账号：十有八九会以为要填正版账号 */}
          <Typography
            sx={{ fontSize: 11.5, color: "text.secondary", lineHeight: 1.65, mt: 1.25 }}
          >
            这是 XCL 自己的账号，只用来连联机节点 —— 和 Minecraft 账号无关。
          </Typography>

          <Stack sx={{ gap: 1, mt: 1 }}>
            <TextField
              label="用户名"
              size="small"
              fullWidth
              autoFocus
              value={username}
              disabled={busy}
              onChange={(event) => setUsername(event.target.value)}
              onKeyDown={onEnter}
              slotProps={{ htmlInput: { maxLength: 24 } }}
            />
            <TextField
              label="密码"
              type="password"
              size="small"
              fullWidth
              value={password}
              disabled={busy}
              onChange={(event) => setPassword(event.target.value)}
              onKeyDown={onEnter}
              helperText={registering ? "至少 8 位，忘了只能找管理员重置" : undefined}
            />
          </Stack>

          {/* 注册比登录多两块：整块收放，卡片高度跟着平滑变化 */}
          <Reveal open={registering}>
            <Stack sx={{ gap: 1, mt: 1 }}>
              <TextField
                label="确认密码"
                type="password"
                size="small"
                fullWidth
                value={confirm}
                disabled={busy || !registering}
                error={mismatch}
                onChange={(event) => setConfirm(event.target.value)}
                onKeyDown={onEnter}
                helperText={mismatch ? "两次输入不一致" : undefined}
              />
              <Box sx={{ display: "flex", alignItems: "center" }}>
                <Checkbox
                  size="small"
                  checked={agreed}
                  disabled={busy || !registering}
                  onChange={(event) => setAgreed(event.target.checked)}
                  sx={{ p: 0.5, ml: -0.5 }}
                />
                <Typography sx={{ fontSize: 12, color: "text.secondary", lineHeight: 1.6 }}>
                  我已阅读并同意
                  <Link
                    component="button"
                    type="button"
                    underline="hover"
                    onClick={() => setTermsOpen(true)}
                    sx={{ fontSize: 12, verticalAlign: "baseline" }}
                  >
                    《联机服务使用须知》
                  </Link>
                </Typography>
              </Box>
            </Stack>
          </Reveal>

          <Button
            fullWidth
            variant="contained"
            sx={{ ...BUTTON_SX, mt: 1.5 }}
            disabled={!canSubmit}
            onClick={() => void submit()}
          >
            {busy
              ? registering
                ? "正在注册…"
                : "正在登录…"
              : registering
                ? "注册并登录"
                : "登录"}
          </Button>
          {hint && (
            <Typography sx={{ fontSize: 12, color: "error.main", lineHeight: 1.6, mt: 1 }}>
              {hint}
            </Typography>
          )}
        </DialogContent>
        <DialogActions sx={{ px: 3, pb: 2, pt: 1 }}>
          <Button onClick={onClose} disabled={busy}>
            取消
          </Button>
        </DialogActions>
      </Dialog>

      <TermsDialog open={termsOpen} onClose={() => setTermsOpen(false)} />
    </>
  );
}

/**
 * 联机服务使用须知。
 *
 * 一屏能读完的四条，不做法律长文 —— 这份东西的作用是「让人知道自己在同意什么」，
 * 不是免责条款。真要被追责，条款写多长都没用；说清楚反而更容易被遵守。
 */
function TermsDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const items: [string, string][] = [
    [
      "这是小范围联机服务",
      "节点是我们自己架的，尽力保证能用，但不承诺 24 小时在线，也不承诺连接速度。",
    ],
    [
      "账号里存了什么",
      "用户名、密码的哈希、注册与登录时间。密码原文我们看不到，也不存。",
    ],
    [
      "不要拿它干别的",
      "别用联机通道做违法的事，别扫节点、别试别人的房间密码。发现会直接封号。",
    ],
    ["你可以要求删号", "找管理员说一声就删，不用理由，删完再注册也行。"],
  ];

  return (
    <Dialog open={open} onClose={onClose} maxWidth="xs" fullWidth>
      <DialogTitle sx={{ fontSize: 15, fontWeight: 700, pb: 0.5 }}>
        联机服务使用须知
      </DialogTitle>
      <DialogContent>
        <Stack sx={{ gap: 1.25, mt: 0.5 }}>
          {items.map(([title, body]) => (
            <Box key={title}>
              <Typography sx={{ fontSize: 13, fontWeight: 600, lineHeight: 1.6 }}>
                {title}
              </Typography>
              <Typography sx={{ fontSize: 12, color: "text.secondary", lineHeight: 1.7 }}>
                {body}
              </Typography>
            </Box>
          ))}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button size="small" onClick={onClose}>
          知道了
        </Button>
      </DialogActions>
    </Dialog>
  );
}
