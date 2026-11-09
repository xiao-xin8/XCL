import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  Avatar,
  Box,
  Button,
  Chip,
  List,
  ListItem,
  ListItemButton,
  ListItemText,
  Paper,
  Stack,
  ToggleButton,
  ToggleButtonGroup,
  Tooltip,
  Typography,
  alpha,
} from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import AddIcon from "@mui/icons-material/Add";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutlined";
import RefreshIcon from "@mui/icons-material/Refresh";
import SwapHorizIcon from "@mui/icons-material/SwapHoriz";
import type {
  AccountView,
  AppError,
  SkinModel,
  SkinRender,
} from "../lib/types";
import { ACCOUNT_KIND_LABEL, toAppError } from "../lib/types";
import { formatDateTime } from "../lib/format";
import { playEnter } from "../motion/pageMotion";
import PageTransition from "../motion/PageTransition";
import { PANEL_SURFACE_SX } from "../lib/surfaces";
import AccountAddDialog from "./AccountAddDialog";
import ErrorState from "./ErrorState";
import { PreviewBox, SkinPlaceholder } from "./SkinPreview";

/** 详情卡用的登录方式全称（列表 Chip 用 ACCOUNT_KIND_LABEL 的短标签）。 */
function kindLongLabel(kind: AccountView["kind"]): string {
  if (kind === "microsoft") return "微软正版";
  return kind === "offline" ? "离线登录" : "第三方登录";
}

/**
 * 账号管理（整页子页，从启动页「管理账号」进入，标题栏给返回键）。
 *
 * 左栏是账号列表（点选）+「添加账号」按钮；右栏**全是选中账号的角色
 * 详细信息**（预览 / 信息 / 操作），添加账号一律走弹窗（`AccountAddDialog`，
 * 三种登录方式一窗三档：微软正版 / 离线 / 第三方）。
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
  /** 添加账号弹窗 */
  const [addOpen, setAddOpen] = useState(false);

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
              还没有账号，点下面的按钮添加一个。
            </Typography>
          )}
        </Box>
        <Button
          fullWidth
          variant="outlined"
          startIcon={<AddIcon sx={{ fontSize: 17 }} />}
          onClick={() => setAddOpen(true)}
          sx={{ mt: 1.25, flexShrink: 0 }}
        >
          添加账号
        </Button>
      </Box>

      {/* 右栏：选中账号的角色详细信息（添加账号走弹窗，不占这栏）。
          换账号的切换动画交给内层 `PageTransition`（同联机页「入口↔房间」）：
          旧卡退场 → 换人 → 新卡落下，皮肤预览随新卡重新淡入。 */}
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
        {loaded && (
          <PageTransition
            pageKey={selected?.id ?? "none"}
            initial={false}
            render={(key) => {
              // 退场期间按旧 key 渲染旧账号；删掉的账号兜底成占位文案
              const shown = accounts.find((a) => a.id === key) ?? null;
              return (
                // 滚动容器：不标记、不参与动画，只负责滚（同联机页）
                <Box
                  sx={{
                    flex: 1,
                    minHeight: 0,
                    overflow: "auto",
                    scrollbarGutter: "stable",
                    display: "flex",
                    flexDirection: "column",
                    gap: 2,
                    "& > *": { flexShrink: 0 },
                  }}
                >
                  {shown ? (
                    <AccountDetail account={shown} onChanged={reload} />
                  ) : (
                    <Paper
                      variant="outlined"
                      sx={{ p: 3, borderRadius: 1, textAlign: "center", color: "text.secondary", fontSize: 13 }}
                    >
                      在左边选一个账号查看详细信息。
                    </Paper>
                  )}
                </Box>
              );
            }}
          />
        )}
      </Box>

      <AccountAddDialog
        open={addOpen}
        onClose={() => setAddOpen(false)}
        onDone={async () => {
          await reload();
          setAddOpen(false);
        }}
      />
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
                label={ACCOUNT_KIND_LABEL[account.kind]}
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
    <>
      <Paper
        variant="outlined"
        sx={{ p: 2.5, borderRadius: 1, display: "flex", gap: 2.5, alignItems: "flex-start" }}
      >
      {/* 3D 皮肤预览：皮肤没到显示方块脸占位，到了进预览盒。
          两件套与登录侧栏同一份组件（`SkinPreview`）；viewer 带 key，
          每个账号重新淡入一次（SKIN_FADE）。 */}
      <Box
        sx={{
          width: 220,
          height: 240,
          flexShrink: 0,
          display: "flex",
          flexDirection: "column",
        }}
      >
        {skin ? <PreviewBox account={account} skin={skin} /> : <SkinPlaceholder />}
      </Box>

      {/* 信息 + 操作 */}
      <Box sx={{ flex: 1, minWidth: 0 }}>
        <Stack direction="row" spacing={1} sx={{ alignItems: "center", mb: 0.5 }}>
          <Typography sx={{ fontSize: 17, fontWeight: 700 }} noWrap>
            {account.username}
          </Typography>
          <Chip
            size="small"
            label={kindLongLabel(account.kind)}
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
          {account.kind === "microsoft" && (
            <Typography sx={{ fontSize: 12, color: "text.secondary" }}>
              登录凭据会自动续期，不用重复登录。
            </Typography>
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
          {(account.kind === "authlib" || account.kind === "microsoft") && (
            <Tooltip
              title={
                account.kind === "authlib" && !account.hasPassword
                  ? "登录时没存密码，不能刷新"
                  : "刷新登录凭据"
              }
            >
              <span>
                <Button
                  size="small"
                  variant="outlined"
                  startIcon={<RefreshIcon sx={{ fontSize: 16 }} />}
                  disabled={busy || (account.kind === "authlib" && !account.hasPassword)}
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

      <AccountExtraInfo account={account} skin={skin} />
    </>
  );
}

/**
 * 底下的「详细信息」卡：把角色的细项摊开摆一摆，右栏不会空荡荡。
 * 离线 / 第三方信息少些；微软账号以后的皮肤管理也挂这一片。
 *
 * ⚠️ 所有信息行始终渲染（皮肤还没到就先显示预期值）：条件渲染会让
 * 卡片在皮肤加载完再长高一截，切换账号的落下动画播完又「重载」一下。
 */
function AccountExtraInfo(props: { account: AccountView; skin: SkinRender | null }) {
  const { account, skin } = props;
  // 皮肤来源的预期值：离线必然是内置；其余先按类型显示，皮肤到了再按实际修正
  const skinSource = skin
    ? skin.source === "server"
      ? account.kind === "microsoft"
        ? "Minecraft 官方"
        : "认证服务器"
      : "内置默认皮肤"
    : account.kind === "offline"
      ? "内置默认皮肤"
      : account.kind === "microsoft"
        ? "Minecraft 官方"
        : "认证服务器";
  return (
    <Paper variant="outlined" sx={{ p: 2.5, borderRadius: 1 }}>
      <Typography sx={{ fontSize: 11.5, fontWeight: 600, color: "text.secondary", mb: 1 }}>
        详细信息
      </Typography>
      <Stack sx={{ "& > *": { py: 0.35 } }}>
        <InfoRow label="UUID" value={account.uuid} mono />
        <InfoRow
          label="登录方式"
          value={kindLongLabel(account.kind)}
        />
        <InfoRow
          label="添加时间"
          value={formatDateTime(new Date(account.createdAtMs).toISOString())}
        />
        {account.kind === "authlib" && (
          <>
            <InfoRow label="认证服务器" value={account.serverUrl ?? "—"} mono />
            <InfoRow label="登录邮箱" value={account.email ?? "—"} />
            <InfoRow
              label="密码"
              value={account.hasPassword ? "已保存（可自动刷新）" : "未保存（刷新需重新登录）"}
            />
          </>
        )}
        {account.kind === "offline" && (
          <InfoRow
            label="皮肤模型"
            value={account.skinModel === "slim" ? "Alex（细手臂）" : account.skinModel === "default" ? "Steve（粗手臂）" : "按 UUID 自动"}
          />
        )}
        <InfoRow label="皮肤来源" value={skinSource} />
      </Stack>

      {account.kind === "microsoft" && (
        <Box sx={{ mt: 1.5, pt: 1.5, borderTop: "1px solid", borderColor: "divider" }}>
          <Typography sx={{ fontSize: 11.5, fontWeight: 600, color: "text.secondary", mb: 0.5 }}>
            皮肤与披风
          </Typography>
          <Typography sx={{ fontSize: 12, color: "text.secondary", lineHeight: 1.7 }}>
            皮肤和披风来自 Minecraft 官方。去官网换皮肤、换披风，这里会自动跟着变。
            在启动器里直接管理皮肤的功能在做了，先在这里留个位置。
          </Typography>
        </Box>
      )}
      {account.kind === "offline" && (
        <Box sx={{ mt: 1.5, pt: 1.5, borderTop: "1px solid", borderColor: "divider" }}>
          <Typography sx={{ fontSize: 12, color: "text.secondary", lineHeight: 1.7 }}>
            离线皮肤靠启动参数里的 UUID 区分模型；切换 Steve / Alex 后进入游戏的
            UUID 会变，单人存档里的玩家数据可能对不上。
          </Typography>
        </Box>
      )}
      {account.kind === "authlib" && (
        <Box sx={{ mt: 1.5, pt: 1.5, borderTop: "1px solid", borderColor: "divider" }}>
          <Typography sx={{ fontSize: 12, color: "text.secondary", lineHeight: 1.7 }}>
            皮肤和模型由认证服务器决定，去服务器的用户中心更换。
          </Typography>
        </Box>
      )}
    </Paper>
  );
}

/** 详细信息卡里的一行「标签：值」。 */
function InfoRow(props: { label: string; value: string; mono?: boolean }) {
  const { label, value, mono } = props;
  return (
    <Box sx={{ display: "flex", gap: 1.5 }}>
      <Typography
        sx={{ fontSize: 12, color: "text.secondary", width: 88, flexShrink: 0, lineHeight: 1.7 }}
      >
        {label}
      </Typography>
      <Typography
        sx={{
          fontSize: 12,
          lineHeight: 1.7,
          wordBreak: "break-all",
          fontFamily: mono ? "ui-monospace, monospace" : undefined,
        }}
      >
        {value}
      </Typography>
    </Box>
  );
}


