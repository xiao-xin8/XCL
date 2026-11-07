import { lazy, Suspense, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  Avatar,
  Box,
  Button,
  Checkbox,
  Chip,
  List,
  ListItem,
  ListItemButton,
  ListItemText,
  Paper,
  Stack,
  TextField,
  ToggleButton,
  ToggleButtonGroup,
  Tooltip,
  Typography,
  alpha,
} from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutlined";
import RefreshIcon from "@mui/icons-material/Refresh";
import SwapHorizIcon from "@mui/icons-material/SwapHoriz";
import type {
  AccountView,
  AppError,
  AuthlibLoginOutcome,
  AuthlibProfile,
  SkinModel,
  SkinRender,
} from "../lib/types";
import { toAppError } from "../lib/types";
import { playEnter } from "../motion/pageMotion";
import { PANEL_SURFACE_SX } from "../lib/surfaces";
import ErrorState from "./ErrorState";
import Loading from "./Loading";

// 和 LoginSidebar 一样：skinview3d 拖着一整个 three.js，只能按需拉。
// 这里是静态 import 的话，three 会跟着启动页一起进首屏 chunk —— 哪怕
// 从头到尾没点进过「管理账号」。
const SkinViewer3D = lazy(() => import("./SkinViewer3D"));

/**
 * 账号管理（整页子页，从启动页「管理账号」进入，标题栏给返回键）。
 *
 * 左栏是账号列表（点选），右栏是选中账号的 3D 皮肤预览 + 操作
 * （切换 / 刷新 / 删除）+ 添加表单。两类登录（微软登录不在范围内，
 * 需要正版账号才能验证）：
 * - 离线：只要一个名字，UUID 由名字派生，不联网；
 * - 第三方：Yggdrasil 协议（LittleSkin 等），服务器地址 + 邮箱 + 密码。
 *
 * 表单校验与错误文案全部来自后端，前端只收集输入、渲染结果。
 *
 * 页面切换与入场动画交给外层 `LaunchPage` 的 `PageTransition`；
 * 这里的列表 / 卡片是数据到了才挂载的，所以数据渲染完的那一次
 * `useLayoutEffect` 里补一次 `playEnter`（扫块时机见下方注释）。
 */
export default function AccountPage(props: {
  /** 当前账号变了（新增 / 切换 / 删除）时通知外部刷新 */
  onChanged?: (active: AccountView | null) => void;
}) {
  const { onChanged } = props;
  /** 回调放 ref：外部传的是内联箭头函数，直接进依赖会把 reload 变成每渲染一次 */
  const changedRef = useRef(onChanged);
  changedRef.current = onChanged;

  const [accounts, setAccounts] = useState<AccountView[]>([]);
  const [loadError, setLoadError] = useState<AppError | null>(null);
  const [loaded, setLoaded] = useState(false);
  /** 左栏选中的账号（右栏预览谁） */
  const [selectedId, setSelectedId] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      const list = await invoke<AccountView[]>("account_list");
      setAccounts(list);
      setLoadError(null);
      changedRef.current?.(list.find((a) => a.active) ?? null);
      setSelectedId((prev) => {
        // 选中的被删了就跟到当前账号；没选过就选当前账号
        if (prev && list.some((a) => a.id === prev)) return prev;
        return list.find((a) => a.active)?.id ?? list[0]?.id ?? null;
      });
    } catch (e: unknown) {
      setLoadError(toAppError(e, "读取账号失败"));
    } finally {
      // 失败也要收闸：否则转圈会一直挂着，错误条反而看不到
      setLoaded(true);
    }
  }, []);

  // 页面挂载即读一次（全屏页没有 open 开关，进页面就是"打开"）
  useEffect(() => {
    void reload();
  }, [reload]);

  const selected =
    accounts.find((a) => a.id === selectedId) ??
    accounts.find((a) => a.active) ??
    null;

  /**
   * 数据到达后补入场。
   *
   * 必须在 layoutEffect 里、且放在数据渲染完的那一次提交之后：
   * 卡片是 `loaded` 之后才挂载的，用「ref 依赖 loaded」那种写法时回调会在
   * 子节点插入前触发，扫不到卡片。这里用 effect —— 它一定跑在 DOM 提交之后，
   * 三张卡和左栏行都已在树里，`playEnter` 能一次收齐。
   *
   * 不要套 `PageLoaderGate`：它会在 400ms 后把子节点藏起来再放出来，
   * 那一次重挂不会触发这个 effect，卡片就会「凭空出现」（Mod 详情页踩过）。
   */
  const rootRef = useRef<HTMLDivElement>(null);
  const playedRef = useRef(false);
  useLayoutEffect(() => {
    if (!loaded || playedRef.current) return;
    const root = rootRef.current;
    if (!root) return;
    playedRef.current = true;
    playEnter(root);
  }, [loaded]);

  return (
    <Box ref={rootRef} sx={{ flex: 1, minHeight: 0, display: "flex" }}>
      {/* 左栏：账号列表。贴边 + 和设置页左栏同一层底 */}
      <Box
        sx={{
          width: 300,
          flexShrink: 0,
          borderRight: "1px solid",
          borderColor: "divider",
          p: 1.5,
          display: "flex",
          flexDirection: "column",
          ...PANEL_SURFACE_SX,
        }}
      >
        <Typography sx={{ fontSize: 11.5, fontWeight: 600, color: "text.secondary", mb: 1 }}>
          我的账号（{accounts.length}）
        </Typography>
        <Box sx={{ flex: 1, minHeight: 0, overflow: "auto", scrollbarGutter: "stable" }}>
          <List
            disablePadding
            data-page-stagger="left"
            sx={{ "& .MuiListItem-root:not(:last-child)": { mb: 1 } }}
          >
            {accounts.map((account) => (
              <AccountRow
                key={account.id}
                account={account}
                selected={account.id === selected?.id}
                onSelect={() => setSelectedId(account.id)}
              />
            ))}
          </List>
          {loaded && accounts.length === 0 && (
            <Typography sx={{ fontSize: 12, color: "text.secondary" }}>
              还没有账号，在右边添加一个。
            </Typography>
          )}
        </Box>
      </Box>

      {/* 右栏：预览 + 操作 + 添加表单 */}
      <Box
        sx={{
          flex: 1,
          minWidth: 0,
          p: 2.5,
          display: "flex",
          flexDirection: "column",
        }}
      >
        {loadError && (
          <ErrorState variant="banner" error={loadError} sx={{ mb: 2 }} />
        )}
        {/* 读完才挂载：数据通常比页面切换的 110ms 还快，先渲染一遍会和
            数据到达后的那次 `playEnter` 各动一次（同一批表单闪两下） */}
        {loaded && (
          <Box
            sx={{
              flex: 1,
              minHeight: 0,
              overflow: "auto",
              scrollbarGutter: "stable",
              display: "flex",
              flexDirection: "column",
              gap: 2,
              // 列方向的卡片默认会被压扁到容器高度，内容就截断了
              "& > *": { flexShrink: 0 },
            }}
          >
            {/* 三张卡：角色信息 / 添加离线账号 / 添加第三方账号。
                都是 Paper，`playEnter` 会自动把每张各算一块，逐块落下 */}
            {selected ? (
              <AccountDetail account={selected} onChanged={reload} />
            ) : (
              <Paper
                variant="outlined"
                sx={{ p: 3, borderRadius: 1, textAlign: "center", color: "text.secondary", fontSize: 13 }}
              >
                在左边选一个账号，或直接在下面添加。
              </Paper>
            )}

            <OfflineForm onDone={reload} />

            <AuthlibForm onDone={reload} />
          </Box>
        )}
      </Box>
    </Box>
  );
}

/** 左栏的账号条目：纯选择，操作都在右栏。 */
function AccountRow(props: {
  account: AccountView;
  selected: boolean;
  onSelect: () => void;
}) {
  const { account, selected, onSelect } = props;
  return (
    <ListItem disablePadding>
      <ListItemButton
        selected={selected}
        onClick={onSelect}
        sx={{
          borderRadius: 1,
          px: 1.25,
          py: 1,
          gap: 1,
          border: "1px solid",
          borderColor: selected ? "brand.ink" : "divider",
          bgcolor: "transparent",
          "&.Mui-selected": {
            bgcolor: (t) => alpha(t.palette.primary.main, 0.08),
            "&:hover": { bgcolor: (t) => alpha(t.palette.primary.main, 0.12) },
          },
          "&:hover": { bgcolor: (t) => alpha(t.palette.primary.main, 0.05) },
        }}
      >
        <Avatar
          variant="rounded"
          sx={{
            width: 32,
            height: 32,
            borderRadius: 1,
            bgcolor: "primary.main",
            color: "primary.contrastText",
            fontSize: 14,
            fontWeight: 700,
            flexShrink: 0,
          }}
        >
          {account.username.slice(0, 1).toUpperCase()}
        </Avatar>
        <ListItemText
          primary={
            <Stack direction="row" spacing={0.75} sx={{ alignItems: "center" }}>
              <Typography sx={{ fontSize: 13, fontWeight: 600 }} noWrap>
                {account.username}
              </Typography>
              <Chip
                size="small"
                label={account.kind === "offline" ? "离线" : "第三方"}
                sx={{ fontSize: 10, height: 18 }}
              />
              {account.active && (
                <Chip
                  size="small"
                  color="primary"
                  label="使用中"
                  sx={{ fontSize: 10, height: 18 }}
                />
              )}
            </Stack>
          }
          secondary={
            account.kind === "authlib" && account.serverUrl ? (
              <Typography
                sx={{
                  fontSize: 10.5,
                  color: "text.secondary",
                  fontFamily: "ui-monospace, monospace",
                  whiteSpace: "nowrap",
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                }}
              >
                {account.serverUrl}
              </Typography>
            ) : null
          }
          slotProps={{
            primary: { component: "div" },
            secondary: { component: "div" },
          }}
          sx={{ my: 0 }}
        />
      </ListItemButton>
    </ListItem>
  );
}

/** 右栏上部：选中账号的皮肤预览 + 信息 + 操作。 */
function AccountDetail(props: {
  account: AccountView;
  onChanged: () => void | Promise<void>;
}) {
  const { account, onChanged } = props;
  const [skin, setSkin] = useState<SkinRender | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  /** 删除要按两次，防手滑 */
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [modelBusy, setModelBusy] = useState(false);

  /** 没手动选过就按 UUID 自动算出来的模型显示 */
  const effectiveModel: SkinModel = account.skinModel ?? skin?.model ?? "default";

  // 换账号时重新拉皮肤
  useEffect(() => {
    let disposed = false;
    setSkin(null);
    setConfirmDelete(false);
    invoke<SkinRender>("skin_current", { accountId: account.id })
      .then((render) => {
        if (!disposed) setSkin(render);
      })
      .catch(() => {
        // 没有预览不耽误管理
      });
    return () => {
      disposed = true;
    };
  }, [account.id]);

  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
      await onChanged();
    } catch (e: unknown) {
      setError(toAppError(e, "操作失败"));
    } finally {
      setBusy(false);
    }
  };

  const chooseModel = async (model: SkinModel) => {
    if (model === effectiveModel || modelBusy || busy) return;
    setModelBusy(true);
    setError(null);
    try {
      await invoke("account_set_skin_model", { id: account.id, model });
      // effect 只跟 account.id，换模型不会自动重拉，这里手动刷新预览
      const render = await invoke<SkinRender>("skin_current", { accountId: account.id });
      setSkin(render);
      await onChanged();
    } catch (e: unknown) {
      setError(toAppError(e, "设置默认皮肤失败"));
    } finally {
      setModelBusy(false);
    }
  };

  return (
    <Paper
      variant="outlined"
      sx={{ p: 2.5, borderRadius: 1, display: "flex", gap: 2.5, alignItems: "flex-start" }}
    >
      {/* 3D 皮肤预览 */}
      <Box
        sx={{
          width: 220,
          height: 240,
          flexShrink: 0,
          borderRadius: 2,
          overflow: "hidden",
          bgcolor: (t) => alpha(t.palette.primary.main, 0.14),
        }}
      >
        {skin ? (
          <Suspense
            fallback={
              <Box sx={{ height: "100%", display: "grid", placeItems: "center" }}>
                <Loading />
              </Box>
            }
          >
            <SkinViewer3D skin={skin.skin} cape={skin.cape} model={skin.model} height={240} />
          </Suspense>
        ) : (
          <Box sx={{ height: "100%", display: "grid", placeItems: "center" }}>
            <Loading />
          </Box>
        )}
      </Box>

      {/* 信息 + 操作 */}
      <Box sx={{ flex: 1, minWidth: 0 }}>
        <Stack direction="row" spacing={1} sx={{ alignItems: "center", mb: 0.5 }}>
          <Typography sx={{ fontSize: 17, fontWeight: 700 }} noWrap>
            {account.username}
          </Typography>
          <Chip
            size="small"
            label={account.kind === "offline" ? "离线登录" : "第三方登录"}
            sx={{ fontSize: 10.5, height: 20 }}
          />
          {account.active && (
            <Chip size="small" color="primary" label="使用中" sx={{ fontSize: 10.5, height: 20 }} />
          )}
        </Stack>

        <Stack spacing={0.25} sx={{ mb: 2 }}>
          <Typography sx={{ fontSize: 12, color: "text.secondary" }}>
            UUID：{account.uuid}
          </Typography>
          {account.kind === "authlib" && (
            <>
              <Typography
                sx={{
                  fontSize: 12,
                  color: "text.secondary",
                  fontFamily: "ui-monospace, monospace",
                  wordBreak: "break-all",
                }}
              >
                服务器：{account.serverUrl ?? "—"}
              </Typography>
              <Typography sx={{ fontSize: 12, color: "text.secondary" }}>
                邮箱：{account.email ?? "—"}
                {!account.hasPassword && "（未保存密码，不能刷新）"}
              </Typography>
            </>
          )}
        </Stack>

        {account.kind === "offline" && (
          <Box sx={{ mb: 2 }}>
            <Typography
              sx={{ fontSize: 11.5, fontWeight: 600, color: "text.secondary", mb: 0.75 }}
            >
              默认皮肤
            </Typography>
            <ToggleButtonGroup
              size="small"
              exclusive
              value={effectiveModel}
              onChange={(_, value: SkinModel | null) => {
                if (value) void chooseModel(value);
              }}
              disabled={busy || modelBusy}
              sx={{
                "& .MuiToggleButton-root": {
                  px: 2,
                  py: 0.4,
                  fontSize: 12,
                  textTransform: "none",
                },
              }}
            >
              <ToggleButton value="default">Steve</ToggleButton>
              <ToggleButton value="slim">Alex</ToggleButton>
            </ToggleButtonGroup>
            <Typography sx={{ fontSize: 11, color: "text.secondary", mt: 0.75 }}>
              离线皮肤靠启动参数里的 UUID 区分模型；切换后进入游戏的 UUID 会变，
              单人存档里的玩家数据可能对不上。
            </Typography>
          </Box>
        )}

        {error && <ErrorState variant="banner" error={error} sx={{ mb: 1.5 }} />}

        <Stack direction="row" spacing={1}>
          {!account.active && (
            <Button
              size="small"
              variant="contained"
              startIcon={<SwapHorizIcon sx={{ fontSize: 16 }} />}
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  await invoke("account_set_active", { id: account.id });
                })
              }
            >
              切换到这个账号
            </Button>
          )}
          {account.kind === "authlib" && (
            <Tooltip title={account.hasPassword ? "刷新登录凭据" : "登录时没存密码，不能刷新"}>
              <span>
                <Button
                  size="small"
                  variant="outlined"
                  startIcon={<RefreshIcon sx={{ fontSize: 16 }} />}
                  disabled={busy || !account.hasPassword}
                  onClick={() =>
                    void run(async () => {
                      await invoke("account_refresh", { id: account.id });
                    })
                  }
                >
                  刷新登录
                </Button>
              </span>
            </Tooltip>
          )}
          <Tooltip title={confirmDelete ? "再点一次确认删除" : "删除账号"}>
            <span>
              <Button
                size="small"
                variant="outlined"
                color={confirmDelete ? "error" : "inherit"}
                startIcon={<DeleteOutlineIcon sx={{ fontSize: 16 }} />}
                disabled={busy}
                onClick={() => {
                  if (!confirmDelete) {
                    setConfirmDelete(true);
                    return;
                  }
                  void run(async () => {
                    await invoke("account_remove", { id: account.id });
                  });
                }}
                sx={
                  confirmDelete
                    ? { borderColor: "error.main", color: "error.main" }
                    : undefined
                }
              >
                {confirmDelete ? "确认删除？" : "删除"}
              </Button>
            </span>
          </Tooltip>
        </Stack>
      </Box>
    </Paper>
  );
}

function OfflineForm(props: { onDone: () => void | Promise<void> }) {
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
    <Paper
      component="form"
      variant="outlined"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
      sx={{ p: 2.5, borderRadius: 1 }}
    >
      <Typography sx={{ fontSize: 11.5, fontWeight: 600, color: "text.secondary", mb: 1 }}>
        添加离线账号
      </Typography>
      <Stack direction="row" spacing={1} sx={{ alignItems: "flex-start" }}>
        <TextField
          size="small"
          label="玩家名"
          placeholder="Steve"
          value={name}
          onChange={(e) => setName(e.target.value)}
          error={!!error}
          helperText={error ? `${error.title}：${error.message}` : "3–16 位字母、数字或下划线，不用密码"}
          sx={{ flex: 1, maxWidth: 320 }}
        />
        <Button
          type="submit"
          variant="contained"
          disabled={busy || name.trim().length === 0}
          sx={{ mt: 0.25 }}
        >
          {busy ? "添加中…" : "添加"}
        </Button>
      </Stack>
    </Paper>
  );
}

function AuthlibForm(props: { onDone: () => void | Promise<void> }) {
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
      <Paper variant="outlined" sx={{ p: 2.5, borderRadius: 1 }}>
        <Typography sx={{ fontSize: 11.5, fontWeight: 600, color: "text.secondary", mb: 1 }}>
          选择角色（{pending.serverUrl}）
        </Typography>
        <Stack spacing={1} sx={{ maxWidth: 320 }}>
          {pending.profiles.map((profile) => (
            <Button
              key={profile.id}
              variant="outlined"
              disabled={busy}
              onClick={() => void submit(profile.id)}
              sx={{ justifyContent: "flex-start", textTransform: "none" }}
            >
              {profile.name}
            </Button>
          ))}
        </Stack>
        <Button size="small" sx={{ mt: 1 }} onClick={() => setPending(null)}>
          返回
        </Button>
      </Paper>
    );
  }

  return (
    <Paper
      component="form"
      variant="outlined"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
      sx={{ p: 2.5, borderRadius: 1 }}
    >
      <Typography sx={{ fontSize: 11.5, fontWeight: 600, color: "text.secondary", mb: 1 }}>
        添加第三方账号
      </Typography>
      <Stack spacing={1.25} sx={{ maxWidth: 420 }}>
        <TextField
          size="small"
          label="认证服务器"
          placeholder="https://littleskin.cn/api/yggdrasil"
          value={server}
          onChange={(e) => setServer(e.target.value)}
          error={!!error}
        />
        <TextField
          size="small"
          label="邮箱"
          value={email}
          onChange={(e) => setEmail(e.target.value)}
          error={!!error}
        />
        <TextField
          size="small"
          label="密码"
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          error={!!error}
          helperText={error ? `${error.title}：${error.message}` : undefined}
        />
        <Box sx={{ display: "flex", alignItems: "center" }}>
          <Checkbox
            size="small"
            checked={remember}
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
            disabled={busy || !server.trim() || !email.trim() || !password}
          >
            {busy ? "登录中…" : "登录"}
          </Button>
        </Box>
      </Stack>
    </Paper>
  );
}
