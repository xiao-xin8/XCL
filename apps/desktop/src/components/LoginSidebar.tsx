import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
import {
  Avatar,
  Box,
  Button,
  Chip,
  ListItem,
  ListItemButton,
  ListItemText,
  Stack,
  Typography,
  alpha,
} from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import type { AccountView, LocalVersion, SkinRender } from "../lib/types";
import Splitter from "./Splitter";
import { useSplitterDragging } from "../lib/splitterDrag";
import { BUTTON_SX } from "../lib/buttons";
import { PANEL_SURFACE_SX } from "../lib/surfaces";
import { useLaunch } from "./LaunchProvider";
import Loading from "./Loading";

// three.js 只有「已登录且有皮肤」时才用得上，离线/未登录进来不该为它解析
const SkinViewer3D = lazy(() => import("./SkinViewer3D"));

/**
 * 登录侧边栏（身份 + 启动按钮常驻左栏）。
 *
 * 上下两个半区，中间的分栏也能拖（比例记 localStorage）：
 * - 上半：3D 皮肤预览，撑满半区。没有账号 / 皮肤没拉好 / 正在拖分栏
 *   手柄时显示皮肤方块脸占位（拖手柄会让预览盒每帧变形，那期间不渲染）。
 * - 下半：当前账号摘要 → 账号列表（点一下切换）→「管理账号 / 启动游戏」
 *   （并排两个按钮）→ 状态提示。
 *
 * 添加 / 删除 / 刷新登录都在「管理账号」整页子页里做，这里只留一键切换 ——
 * 功能不重复，侧边栏只回答「现在用谁启动」这一个问题。
 *
 * 没有账号就不许启动：按钮禁用 + 文案说明，后端 `player_profile()` 还有一道
 * 硬校验，双保险。
 *
 * 宽度由父级（LaunchPage 的 Splitter）按比例给，这里撑满即可。
 * 右侧不画 border —— 与主内容区的分隔由父级的 Splitter 常驻线负责。
 */

// ---------- 底部两个按钮的文案宽窄切换 ----------

/**
 * 判定依据是这一行自己的宽度，不是窗口宽度。
 *
 * 侧栏是可拖拽的（占窗口 22% ~ 70%），窗口宽也不代表这两个按钮有地方。
 * 230px 是「管理账号 + 启动游戏」并排不折行的下限。
 */
const ACTIONS_COMPACT_BELOW = 230;

// ---------- 预览 / 账号两半区的分栏比例（localStorage 记忆） ----------

const LOGIN_LAYOUT_KEY = "xcl.loginLayout";

type LoginLayout = {
  /** 上半（皮肤预览）占侧边栏高度的比例 */
  preview: number;
};

const LOGIN_LAYOUT_DEFAULT: LoginLayout = { preview: 0.5 };
/** 钳制放开：要能自由拖，只保留「别把哪格拖没」的最低限度 */
const PREVIEW_MIN = 0.12;
const PREVIEW_MAX = 0.88;

function readLoginLayout(): LoginLayout {
  try {
    const raw = localStorage.getItem(LOGIN_LAYOUT_KEY);
    if (!raw) return LOGIN_LAYOUT_DEFAULT;
    const parsed = JSON.parse(raw) as Partial<LoginLayout>;
    const preview =
      typeof parsed.preview === "number" && Number.isFinite(parsed.preview)
        ? Math.min(PREVIEW_MAX, Math.max(PREVIEW_MIN, parsed.preview))
        : LOGIN_LAYOUT_DEFAULT.preview;
    return { preview };
  } catch {
    // 存储被写坏就当默认值，不拦着用
    return LOGIN_LAYOUT_DEFAULT;
  }
}
export default function LoginSidebar(props: {
  /** 当前选中的版本（启动按钮的目标） */
  selected: LocalVersion | null;
  /** 当前账号（active）；没有就不显示预览、不许启动 */
  account: AccountView | null;
  /** 全部账号（已按创建时间排好，一键切换列表） */
  accounts: AccountView[];
  /** 账号列表 / 当前账号变了（切换 / 外部增删），让父级重新拉 */
  onAccountsChanged: () => void;
  /** 打开账号管理整页子页 */
  onManageAccounts: () => void;
}) {
  const { selected, account, accounts, onAccountsChanged, onManageAccounts } = props;
  const { status, starting, running, start, stop } = useLaunch();

  /** 正在切换的账号 id（行内 loading） */
  const [switchingId, setSwitchingId] = useState<string | null>(null);
  /** 预览 / 账号两半区的比例：改了就落盘，下次启动还原 */
  const [loginLayout, setLoginLayout] = useState<LoginLayout>(readLoginLayout);

  /** 有分栏手柄正在被拖（左右 / 上下 / 预览↔账号都算）：这期间把 3D 预览
   *  换成静态占位 —— 手柄拖动会让预览盒每帧变形，WebGL 重绘只会闪；松手后
   *  3D 重挂、按新尺寸重算。这就是「摁住手柄立马占位、松开刷新」。
   *
   *  只认分栏手柄，不认系统窗口拖动：拖窗口只是整体平移/缩放，预览盒
   *  内部没有逐帧变化，没有闪的问题，占位纯属多余。 */
  const splitterDragging = useSplitterDragging();

  const setPreviewRatio = useCallback((preview: number) => {
    setLoginLayout((prev) => {
      const next = { ...prev, preview };
      localStorage.setItem(LOGIN_LAYOUT_KEY, JSON.stringify(next));
      return next;
    });
  }, []);

  // 皮肤预览数据：当前账号变了就重新拉。没有账号时不拉。
  const [skin, setSkin] = useState<SkinRender | null>(null);
  useEffect(() => {
    if (!account) {
      setSkin(null);
      return;
    }
    let disposed = false;
    invoke<SkinRender>("skin_current")
      .then((render) => {
        if (!disposed) setSkin(render);
      })
      .catch(() => {
        // 拉不到皮肤只是没有预览，不拦着任何东西
      });
    return () => {
      disposed = true;
    };
  }, [account]);

  const switchTo = useCallback(
    async (id: string) => {
      if (account?.id === id || switchingId) return;
      setSwitchingId(id);
      try {
        await invoke("account_set_active", { id });
        onAccountsChanged();
      } catch {
        // 切失败就当没发生，列表还是老样子；错误详情后端已给
      } finally {
        setSwitchingId(null);
      }
    },
    [account?.id, onAccountsChanged, switchingId],
  );

  // ---------- 启动按钮状态 ----------

  const isThisRunning = running && status?.versionId === selected?.id;
  const busyLaunching = starting !== null && starting === selected?.id;
  const launchDisabled =
    !selected || !selected.launchable || !account || busyLaunching || (running && !isThisRunning);

  const launchLabel = isThisRunning
    ? "结束游戏"
    : busyLaunching
      ? "正在准备…"
      : "启动游戏";

  /** 窄的时候用的短文案。宁可缩成两个字，也不要让按钮变成两行。 */
  const launchLabelShort = isThisRunning
    ? "结束"
    : busyLaunching
      ? "准备中"
      : "启动";

  /**
   * 底部按钮那一行够不够宽。
   *
   * 用 `ResizeObserver` 量这一行自己的宽度：侧栏是拖拽出来的，
   * 窗口宽窄和这里的可用空间没有对应关系。
   */
  const actionsRef = useRef<HTMLDivElement>(null);
  const [compactActions, setCompactActions] = useState(false);

  useEffect(() => {
    const el = actionsRef.current;
    if (!el) return;
    const observer = new ResizeObserver((entries) => {
      const width = entries[0]?.contentRect.width ?? 0;
      // 宽度 0 表示还没布局，别在这时候切成短文案（会闪一下）
      const next = width > 0 && width < ACTIONS_COMPACT_BELOW;
      setCompactActions((prev) => (prev === next ? prev : next));
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const onLaunchClick = () => {
    if (isThisRunning) {
      void stop();
    } else if (selected) {
      void start(selected.id).catch(() => {});
    }
  };

  return (
    <Box
      sx={{
        width: "100%",
        height: "100%",
        ...PANEL_SURFACE_SX,
        display: "flex",
        flexDirection: "column",
        minHeight: 0,
      }}
    >
      {/* ===== 上半：皮肤预览（高度 = 比例，可拖） ===== */}
      <Box
        sx={{
          height: `${loginLayout.preview * 100}%`,
          flexShrink: 0,
          minHeight: 0,
          display: "flex",
          flexDirection: "column",
          p: 1.5,
          pb: 0,
        }}
      >
        {/* 没有账号 / 皮肤没拉好 / 正在拖分栏手柄：
            一律显示皮肤方块脸占位，不出现空白、不铺贴图、不写「正在加载」 */}
        {account && skin && !splitterDragging ? (
          <PreviewBox account={account} skin={skin} />
        ) : (
          <SkinPlaceholder badge={account ? undefined : "未登录"} />
        )}
      </Box>

      {/* ===== 中：可拖拽分栏（预览 ↔ 账号区） ===== */}
      <Splitter
        orientation="horizontal"
        value={loginLayout.preview}
        min={PREVIEW_MIN}
        max={PREVIEW_MAX}
        onChange={setPreviewRatio}
        resetValue={LOGIN_LAYOUT_DEFAULT.preview}
      />

      {/* ===== 下半：账号摘要 + 一键切换 + 启动 ===== */}
      <Box
        sx={{
          flex: 1,
          minHeight: 0,
          display: "flex",
          flexDirection: "column",
          p: 1.5,
          gap: 1,
          overflow: "hidden",
        }}
      >
        {/* 当前账号摘要 */}
        <Box sx={{ flexShrink: 0 }}>
          <Stack direction="row" spacing={0.75} sx={{ alignItems: "center", mb: 0.25 }}>
            <Typography sx={{ fontSize: 14, fontWeight: 600 }} noWrap>
              {account?.username ?? "未选择账号"}
            </Typography>
            {account && (
              <Chip
                size="small"
                label={account.kind === "authlib" ? "第三方" : "离线"}
                sx={{ fontSize: 10, height: 18 }}
              />
            )}
          </Stack>
          <Typography sx={{ fontSize: 10.5, color: "text.secondary", lineHeight: 1.5 }} noWrap>
            {account?.kind === "authlib"
              ? (account.serverUrl ?? "第三方登录")
              : account
                ? "离线登录 · 默认 Steve / Alex 皮肤"
                : "启动游戏前需要先添加一个账号"}
          </Typography>
        </Box>

        {/* 账号列表：点一下切换，不用进管理面板（不摆标头，行自己说明自己） */}
        <Box sx={{ flex: 1, minHeight: 0, overflow: "auto", scrollbarGutter: "stable" }}>
          <Stack spacing={0.75}>
            {accounts.map((item) => (
              <AccountSwitchRow
                key={item.id}
                account={item}
                switching={switchingId === item.id}
                onSelect={() => void switchTo(item.id)}
              />
            ))}
          </Stack>
          {accounts.length === 0 && (
            <Typography sx={{ fontSize: 11.5, color: "text.secondary", lineHeight: 1.6 }}>
              还没有账号，点下面的「管理账号」添加一个。
            </Typography>
          )}
        </Box>

        {/* 管理账号（浅色次要）+ 启动游戏（主色主操作）：并排一行，同高不同级。
            宽度不够时缩成两个字，绝不折行 —— 折成「管理账 / 号」那种两行
            会让整块高度乱跳。
            判断依据是这一行自己的宽度（`ResizeObserver` 实测），不是窗口
            宽度：侧栏本身可拖拽，窗口宽不代表这两个按钮有地方。 */}
        <Stack
          ref={actionsRef}
          direction="row"
          spacing={1}
          sx={{ flexShrink: 0 }}
        >
          <Button
            variant="outlined"
            onClick={onManageAccounts}
            sx={{ ...BUTTON_SX, flex: 1, whiteSpace: "nowrap" }}
          >
            {compactActions ? "账号" : "管理账号"}
          </Button>
          <Button
            variant="contained"
            disabled={launchDisabled}
            onClick={onLaunchClick}
            sx={{ ...BUTTON_SX, flex: 1.25, whiteSpace: "nowrap" }}
          >
            {compactActions ? launchLabelShort : launchLabel}
          </Button>
        </Stack>
        {!account && (
          <Typography sx={{ fontSize: 10.5, color: "text.secondary", textAlign: "center" }}>
            先去「管理账号」添加一个账号
          </Typography>
        )}
        {selected && !selected.launchable && (
          <Typography sx={{ fontSize: 10.5, color: "text.secondary", textAlign: "center" }}>
            这个版本还启动不了
          </Typography>
        )}
      </Box>
    </Box>
  );
}

/** 上半区的皮肤预览：容器撑满半区，高度靠实测喂给 skinview3d。 */
function PreviewBox(props: { account: AccountView; skin: SkinRender | null }) {
  const { account, skin } = props;
  const boxRef = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState(0);

  // 预览盒是 flex 撑开的，没有固定像素高；skinview3d 只吃数字，
  // 所以量一下容器实际高度再喂给它（首帧用 220 兜底）。
  useEffect(() => {
    const box = boxRef.current;
    if (!box) return;
    const observer = new ResizeObserver(() => setHeight(box.clientHeight));
    observer.observe(box);
    setHeight(box.clientHeight);
    return () => observer.disconnect();
  }, []);

  return (
    <Box
      sx={{
        flex: 1,
        minHeight: 0,
        borderRadius: 1,
        overflow: "hidden",
        position: "relative",
        bgcolor: (t) => alpha(t.palette.primary.main, 0.14),
      }}
    >
      <Box ref={boxRef} sx={{ position: "absolute", inset: 0 }}>
        {skin ? (
          <Suspense
            fallback={
              <Box sx={{ height: "100%", display: "grid", placeItems: "center" }}>
                <Loading />
              </Box>
            }
          >
            <SkinViewer3D
              key={account.id}
              skin={skin.skin}
              cape={skin.cape}
              model={skin.model}
              height={height || 220}
            />
          </Suspense>
        ) : (
          <Box sx={{ height: "100%", display: "grid", placeItems: "center" }}>
            <Loading />
          </Box>
        )}
      </Box>
    </Box>
  );
}

/** 账号列表行：ListItemButton（MUI 原生涟漪 / 焦点 / 键盘），点一下切换。
 *
 *  不能给行加 disabled：MUI 的 ButtonBase 在 disabled 切换时会立刻掐掉
 *  正在播放的 TouchRipple，点击反馈一闪就没。防重入由 `switchTo` 里的守卫
 *   负责，这里只把「正在切换」表示成行尾一个小转圈。 */
function AccountSwitchRow(props: {
  account: AccountView;
  switching: boolean;
  onSelect: () => void;
}) {
  const { account, switching, onSelect } = props;
  const selected = account.active;
  return (
    <ListItem disablePadding>
      <ListItemButton
        selected={selected}
        onClick={onSelect}
        sx={{
          borderRadius: 1,
          px: 1,
          py: 0.75,
          gap: 1,
          border: "1px solid",
          borderColor: selected ? "brand.ink" : "divider",
          bgcolor: "transparent",
          // 切换中直接压暗整行：行内不再插转圈（全软件只有一种加载指示器，
          // 64px 塞进 30px 高的账号行会把行撑开）
          opacity: switching ? 0.55 : 1,
          pointerEvents: switching ? "none" : "auto",
          transition: "opacity 160ms ease",
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
            width: 30,
            height: 30,
            borderRadius: 1,
            bgcolor: selected ? "primary.main" : "action.selected",
            color: selected ? "primary.contrastText" : "text.secondary",
            fontSize: 13,
            fontWeight: 700,
            flexShrink: 0,
          }}
        >
          {account.username.slice(0, 1).toUpperCase()}
        </Avatar>
        <ListItemText
          primary={
            <Stack direction="row" spacing={0.6} sx={{ alignItems: "center" }}>
              <Typography sx={{ fontSize: 12.5, fontWeight: 600 }} noWrap>
                {account.username}
              </Typography>
              <Chip
                size="small"
                label={account.kind === "offline" ? "离线" : "第三方"}
                sx={{ fontSize: 9.5, height: 16, flexShrink: 0 }}
              />
            </Stack>
          }
          secondary={
            account.kind === "authlib" && account.serverUrl ? (
              <Typography
                sx={{
                  fontSize: 10,
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
        {selected && (
          <Chip
            size="small"
            color="primary"
            label="使用中"
            sx={{ fontSize: 9.5, height: 16, flexShrink: 0 }}
          />
        )}
      </ListItemButton>
    </ListItem>
  );
}

/** 预览占位：皮肤方块脸图标（不是把整张贴图铺出来）。
 *
 *  没有账号 / 皮肤没拉好 / 正在拖分栏手柄时显示。
 *  从皮肤贴图里裁「脸 (8,8)」打底、「头发 overlay (32,8)」盖上，叠成
 *  Minecraft 风味的方块脸 —— 一眼能看出是皮肤相关的占位，而不是一张图。
 *  背景同一张 steve.png，靠 background-position 取不同区域，零额外请求。 */
function SkinPlaceholder(props: { badge?: string }) {
  /** 方块脸边长（px）。脸是 8×8 的格子，放大 12 倍 = 96。 */
  const FACE = 96;
  /** 8px 一格在放大图里的位移 = 8 × 12 = 96；64px 整图 × 12 = 768。 */
  const STEP = FACE / 8;
  return (
    <Box
      sx={{
        flex: 1,
        minHeight: 0,
        borderRadius: 1,
        overflow: "hidden",
        position: "relative",
        bgcolor: (t) => alpha(t.palette.primary.main, 0.14),
        display: "grid",
        placeItems: "center",
      }}
    >
      <Box
        sx={{
          width: FACE,
          height: FACE,
          position: "relative",
          imageRendering: "pixelated",
          // 占位感：比 3D 预览淡一截
          opacity: 0.7,
          pointerEvents: "none",
          userSelect: "none",
          "& > *": {
            position: "absolute",
            inset: 0,
            backgroundImage: "url(/skins/steve.png)",
            backgroundSize: `${STEP * 64}px ${STEP * 64}px`,
          },
        }}
      >
        {/* 脸 */}
        <Box sx={{ backgroundPosition: `-${STEP * 8}px -${STEP * 8}px` }} />
        {/* 头发（皮肤第二层 overlay） */}
        <Box sx={{ backgroundPosition: `-${STEP * 32}px -${STEP * 8}px` }} />
      </Box>
      {props.badge && (
        <Chip
          size="small"
          label={props.badge}
          sx={{
            position: "absolute",
            left: 8,
            top: 8,
            height: 18,
            fontSize: 10,
            // 小徽标要保持可读，不跟界面的透明度一起变
            bgcolor: "surface.solid",
            color: "text.secondary",
          }}
        />
      )}
    </Box>
  );
}
