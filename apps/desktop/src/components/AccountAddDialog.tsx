import { useCallback, useEffect, useRef, useState } from "react";
import {
  Box,
  Button,
  Checkbox,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  Stack,
  TextField,
  Typography,
} from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import ContentCopyIcon from "@mui/icons-material/ContentCopy";
import OpenInNewIcon from "@mui/icons-material/OpenInNew";
import type {
  AppError,
  AuthlibLoginOutcome,
  AuthlibProfile,
  MsaLoginStart,
  MsaPollView,
} from "../lib/types";
import { toAppError } from "../lib/types";
import { BUTTON_SX } from "../lib/buttons";
import PillTabs from "./PillTabs";
import Reveal from "./settings/Reveal";
import ErrorState from "./ErrorState";

/** 顶部三个档位。微软没配 client id 先垫底（用户定的顺序），配好后再提前。 */
type AddTab = "msa" | "offline" | "authlib";

const TABS = [
  { value: "offline", label: "离线账号" },
  { value: "authlib", label: "第三方账号" },
  { value: "msa", label: "微软账号" },
] as const;

/**
 * 微软登录的真 UI 开关：client id 还没注册到手，先整档只显示一句提示。
 * 代码全部留着，拿到 client id 把这里改回 `true` 就恢复。
 */
const MSA_UI_READY: boolean = false;

/** 弹窗比 XCL 账号弹窗（xs）大一号，三列字段摆得开。 */
const DIALOG_MAX = "sm";

/**
 * 添加账号弹窗：三种登录方式一个弹窗装完，顶部胶囊 Tab 切换。
 *
 * 骨架与 `ConnectLoginDialog` 一致（胶囊 Tab / Reveal 收放 / BUTTON_SX /
 * ErrorState），只是档位从「登录/注册」换成三种账号。表单校验与错误文案
 * 全部来自后端，前端只收集输入、渲染结果。
 *
 * 换档不做「掉落入场」——三份内容各包一层 `Reveal`，旧档收、新档放，
 * **弹窗高度跟着内容平滑生长**（和登录弹窗切「登录/注册」同一套）。
 * 收起的档位把控件禁用掉，防止 Tab 键聚焦到看不见的输入框。
 *
 * 微软档是设备代码流：点开始 → 后端取设备码 → 展示验证码并自动开浏览器 /
 * 复制 → 轮询等结果（节奏由后端把关，前端一秒 tick 一次即可）。
 */
export default function AccountAddDialog(props: {
  open: boolean;
  onClose: () => void;
  /** 任一方式添加成功后：外面刷新列表，这里关弹窗。 */
  onDone: () => void | Promise<void>;
}) {
  const { open, onClose, onDone } = props;
  const [tab, setTab] = useState<AddTab>("offline");

  return (
    <Dialog open={open} onClose={onClose} maxWidth={DIALOG_MAX} fullWidth>
      <DialogTitle sx={{ fontSize: 16, fontWeight: 700, pb: 1 }}>添加账号</DialogTitle>
      <DialogContent sx={{ pt: "12px !important" }}>
        <PillTabs items={TABS} value={tab} onChange={setTab} />

        {/* 三份内容都挂着，靠 Reveal 一收一放换档 —— 弹窗高度平滑生长。
            收起的那份 `active=false`，里面的控件禁用防误触。 */}
        <Box sx={{ mt: 1.5 }}>
          <Reveal open={tab === "msa"}>
            <MsaTab active={tab === "msa"} onDone={onDone} />
          </Reveal>
          <Reveal open={tab === "offline"}>
            <OfflineTab active={tab === "offline"} onDone={onDone} />
          </Reveal>
          <Reveal open={tab === "authlib"}>
            <AuthlibTab active={tab === "authlib"} onDone={onDone} />
          </Reveal>
        </Box>
      </DialogContent>
      <DialogActions sx={{ px: 3, pb: 2, pt: 1 }}>
        <Button onClick={onClose}>关闭</Button>
      </DialogActions>
    </Dialog>
  );
}

// ---------- 微软账号（设备代码流） ----------

/** 设备码登录的三段：没开始 / 等输码 / 已有结果收尾。 */
type MsaPhase = "idle" | "waiting";

function MsaTab(props: { active: boolean; onDone: () => void | Promise<void> }) {
  const { active, onDone } = props;
  const [phase, setPhase] = useState<MsaPhase>("idle");
  const [code, setCode] = useState<MsaLoginStart | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  const [copied, setCopied] = useState(false);

  /** begin 是异步的，期间用户可能切走 —— 用 ref 兜住「回来前别进等待态」 */
  const activeRef = useRef(active);
  activeRef.current = active;

  /** 取消进行中的登录（切档 / 关弹窗 / 手动取消都要走到这里）。
   *  验证码内容留在 Reveal 里跟着一起收（清空会瞬间消失不跟动画），下次开始登录会换新。 */
  const cancelMsa = useCallback(() => {
    void invoke("account_msa_cancel").catch(() => undefined);
    setPhase("idle");
    setCopied(false);
  }, []);

  // 切走档位（内容靠 Reveal 收起、不卸载）就把登录取消掉，别在后台空转轮询
  useEffect(() => {
    if (!active) cancelMsa();
  }, [active, cancelMsa]);

  const begin = useCallback(async () => {
    setBusy(true);
    setError(null);
    setCopied(false);
    try {
      const start = await invoke<MsaLoginStart>("account_msa_begin");
      if (!activeRef.current) return;
      setCode(start);
      setPhase("waiting");
      // PCL 同款：拿到验证码就自动开浏览器 + 自动复制，少两步手工
      void openUrl(start.verificationUri).catch(() => undefined);
      void navigator.clipboard.writeText(start.userCode).catch(() => undefined);
      setCopied(true);
    } catch (e: unknown) {
      if (!activeRef.current) return;
      setError(toAppError(e, "开始登录失败"));
    } finally {
      setBusy(false);
    }
  }, []);

  // 等输码期间每秒 tick 一次；节奏（间隔 / slow_down）由后端把关
  useEffect(() => {
    if (phase !== "waiting") return;
    let disposed = false;
    const tick = async () => {
      try {
        const result = await invoke<MsaPollView>("account_msa_poll");
        if (disposed) return;
        if (result.status === "done") {
          setPhase("idle");
          setCode(null);
          await onDone();
        }
      } catch (e: unknown) {
        if (disposed) return;
        const appError = toAppError(e, "登录失败");
        setError(appError);
        // 后端把状态清了的（终止性错误）回到起点；可重试的（链上网络
        // 抖动，微软令牌还在）留在 waiting，下一轮 tick 自动接着跑
        if (!appError.retryable) {
          cancelMsa();
        }
      }
    };
    const timer = window.setInterval(() => void tick(), 1000);
    void tick();
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [phase, onDone, cancelMsa]);

  const copyCode = () => {
    if (!code) return;
    void navigator.clipboard
      .writeText(code.userCode)
      .then(() => setCopied(true))
      .catch(() => undefined);
  };

  // 真 UI 先不露脸（代码留在下面，一行没删）：client id 还没弄到手
  if (!MSA_UI_READY) {
    return (
      <Typography sx={{ fontSize: 12, color: "text.secondary", lineHeight: 1.7 }}>
        哈哈哈，作者还没有弄到那个 client id，这功能还用不了。
      </Typography>
    );
  }

  return (
    <Stack spacing={1.25}>
      {phase === "idle" && (
        <>
          <Typography sx={{ fontSize: 12, color: "text.secondary", lineHeight: 1.7 }}>
            用微软账号登录正版 Minecraft。点开始后会打开微软的登录页，
            在网页里输入启动器给出的验证码就完成登录，不用把密码交给启动器。
          </Typography>
          <Box>
            <Button
              variant="contained"
              sx={BUTTON_SX}
              disabled={busy || !active}
              onClick={() => void begin()}
            >
              {busy ? "正在获取验证码…" : "开始登录"}
            </Button>
          </Box>
        </>
      )}

      {/* 等输码：验证码整块收放，卡片高度平滑变化（同登录弹窗的注册块） */}
      <Reveal open={phase === "waiting"}>
        {code && (
          <Stack spacing={1.25} sx={{ mt: 0.25 }}>
            <Typography sx={{ fontSize: 12, color: "text.secondary", lineHeight: 1.7 }}>
              登录网页已自动打开，请在网页中输入下面的验证码。
              也可以用其他设备打开 {code.verificationUri} 输入。
            </Typography>

            {/* 验证码大字：点一下也能复制 */}
            <Box
              onClick={copyCode}
              sx={{
                py: 1.25,
                borderRadius: 1,
                border: "1px solid",
                borderColor: "divider",
                textAlign: "center",
                cursor: "pointer",
                "&:hover": { borderColor: "brand.ink" },
              }}
            >
              <Typography
                sx={{
                  fontSize: 30,
                  fontWeight: 700,
                  letterSpacing: 6,
                  fontFamily: "ui-monospace, monospace",
                  userSelect: "all",
                }}
              >
                {code.userCode}
              </Typography>
              <Typography sx={{ fontSize: 11, color: "text.secondary", mt: 0.25 }}>
                {copied ? "已复制" : "点击复制验证码"}
              </Typography>
            </Box>

            <Stack direction="row" spacing={1}>
              <Button
                size="small"
                variant="outlined"
                disabled={!active || phase !== "waiting"}
                startIcon={<OpenInNewIcon sx={{ fontSize: 16 }} />}
                onClick={() => void openUrl(code.verificationUri).catch(() => undefined)}
              >
                重新打开网页
              </Button>
              <Button
                size="small"
                variant="outlined"
                disabled={!active || phase !== "waiting"}
                startIcon={<ContentCopyIcon sx={{ fontSize: 16 }} />}
                onClick={copyCode}
              >
                复制验证码
              </Button>
              <Box sx={{ flex: 1 }} />
              <Button
                size="small"
                color="inherit"
                disabled={!active || phase !== "waiting"}
                onClick={cancelMsa}
              >
                取消
              </Button>
            </Stack>

            <Typography sx={{ fontSize: 11.5, color: "text.secondary", lineHeight: 1.7 }}>
              {code.message}
            </Typography>
            <Typography sx={{ fontSize: 11.5, color: "text.secondary", lineHeight: 1.7 }}>
              登录成功后会自动把这个账号加进来，这里不用刷新。
            </Typography>
          </Stack>
        )}
      </Reveal>

      {error && <ErrorState variant="banner" error={error} />}
    </Stack>
  );
}

// ---------- 离线账号 ----------

function OfflineTab(props: { active: boolean; onDone: () => void | Promise<void> }) {
  const { active } = props;
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<AppError | null>(null);

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      await invoke("account_add_offline", { name: name.trim() });
      setName("");
      await props.onDone();
    } catch (e: unknown) {
      setError(toAppError(e, "添加失败"));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Stack
      component="form"
      spacing={1.25}
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <Typography sx={{ fontSize: 12, color: "text.secondary", lineHeight: 1.7 }}>
        离线登录只需要一个玩家名，不联网、不验证身份。
        同一个名字在任何启动器里的 UUID 都相同，换启动器存档不丢。
      </Typography>
      <TextField
        size="small"
        label="玩家名"
        placeholder="Steve"
        fullWidth
        value={name}
        disabled={busy || !active}
        onChange={(e) => setName(e.target.value)}
        error={!!error}
        helperText={
          error ? `${error.title}：${error.message}` : "3–16 位字母、数字或下划线，不用密码"
        }
      />
      <Box>
        <Button
          type="submit"
          variant="contained"
          sx={BUTTON_SX}
          disabled={busy || !active || name.trim().length === 0}
        >
          {busy ? "添加中…" : "添加"}
        </Button>
      </Box>
    </Stack>
  );
}

// ---------- 第三方账号（Yggdrasil） ----------

function AuthlibTab(props: { active: boolean; onDone: () => void | Promise<void> }) {
  const { active } = props;
  const [server, setServer] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [remember, setRemember] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  /** 多角色时要选一个 */
  const [pending, setPending] = useState<{
    profiles: AuthlibProfile[];
    serverUrl: string;
  } | null>(null);

  const submit = async (profileId?: string) => {
    setBusy(true);
    setError(null);
    try {
      const outcome = await invoke<AuthlibLoginOutcome>("account_add_authlib", {
        serverUrl: server.trim(),
        email: email.trim(),
        password,
        profileId: profileId ?? null,
        rememberPassword: remember,
      });
      if (outcome.outcome === "needProfile") {
        setPending({ profiles: outcome.profiles, serverUrl: outcome.serverUrl });
      } else {
        setPassword("");
        setPending(null);
        await props.onDone();
      }
    } catch (e: unknown) {
      setError(toAppError(e, "登录失败"));
    } finally {
      setBusy(false);
    }
  };

  if (pending) {
    return (
      <Stack spacing={1}>
        <Typography sx={{ fontSize: 11.5, fontWeight: 600, color: "text.secondary" }}>
          选择角色（{pending.serverUrl}）
        </Typography>
        <Stack spacing={1} sx={{ maxWidth: 320 }}>
          {pending.profiles.map((profile) => (
            <Button
              key={profile.id}
              variant="outlined"
              disabled={busy || !active}
              onClick={() => void submit(profile.id)}
              sx={{ justifyContent: "flex-start", textTransform: "none" }}
            >
              {profile.name}
            </Button>
          ))}
        </Stack>
        <Box>
          <Button size="small" disabled={!active} onClick={() => setPending(null)}>
            返回
          </Button>
        </Box>
      </Stack>
    );
  }

  return (
    <Stack
      component="form"
      spacing={1.25}
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <Typography sx={{ fontSize: 12, color: "text.secondary", lineHeight: 1.7 }}>
        登录 LittleSkin、Blessing Skin 这类 Yggdrasil 认证服务器。
        服务器地址填 API 根，不带 /authserver。
      </Typography>
      <TextField
        size="small"
        label="认证服务器"
        placeholder="https://littleskin.cn/api/yggdrasil"
        fullWidth
        value={server}
        disabled={busy || !active}
        onChange={(e) => setServer(e.target.value)}
        error={!!error}
      />
      <Box>
        <Button
          size="small"
          variant="outlined"
          disabled={busy || !active}
          onClick={() => setServer("https://littleskin.cn/api/yggdrasil")}
        >
          设置为 LittleSkin
        </Button>
      </Box>
      <TextField
        size="small"
        label="邮箱"
        fullWidth
        value={email}
        disabled={busy || !active}
        onChange={(e) => setEmail(e.target.value)}
        error={!!error}
      />
      <TextField
        size="small"
        label="密码"
        type="password"
        fullWidth
        value={password}
        disabled={busy || !active}
        onChange={(e) => setPassword(e.target.value)}
        error={!!error}
        helperText={error ? `${error.title}：${error.message}` : undefined}
      />
      <Box sx={{ display: "flex", alignItems: "center" }}>
        <Checkbox
          size="small"
          checked={remember}
          disabled={busy || !active}
          onChange={(e) => setRemember(e.target.checked)}
          sx={{ p: 0.5, ml: -0.5 }}
        />
        <Typography sx={{ fontSize: 12, color: "text.secondary" }}>
          记住密码（之后可以一键刷新登录，不用重新输）
        </Typography>
      </Box>
      <Box>
        <Button
          type="submit"
          variant="contained"
          sx={BUTTON_SX}
          disabled={busy || !active || !server.trim() || !email.trim() || !password}
        >
          {busy ? "登录中…" : "登录"}
        </Button>
      </Box>
    </Stack>
  );
}
