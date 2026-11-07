import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Box, Button, Stack, Typography } from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { toAppError, type AppError } from "../lib/types";
import ConnectLoginDialog, {
  type ConnectAccount,
  type ConnectAuthMode,
} from "./ConnectLoginDialog";
import ErrorState from "./ErrorState";
import Loading from "./Loading";
import { BUTTON_SX } from "../lib/buttons";
import { useConfig } from "../lib/config";
import { useRegisterNestedPage } from "../lib/nestedNav";
import PageTransition from "../motion/PageTransition";
import { playEnter } from "../motion/pageMotion";
import SettingGroup from "./settings/SettingGroup";
import { EntryCards } from "./connect/EntryCards";
import { ModuleCard } from "./connect/ModuleCard";
import { SessionCard } from "./connect/SessionCard";
import { CreateRoomCard, JoinRoomCard } from "./connect/RoomCards";
import { MODULE_TASK_PREFIX, sessionActive } from "./connect/types";
import type {
  ConnectModuleStatus,
  ConnectState,
  ConnectView,
  InstallTask,
} from "./connect/types";

// 数据形状、模块级常量、会话判定都在 `./connect/types.ts`；
// 各种卡片在 `./connect/` 下。本文件只留页面状态机（视图切换 + 轮询）。

/**
 * 上次停在哪一页。
 *
 * 放模块级：切到别的 tab 时页面组件会被卸载，`useState` 会丢，
 * 切回来就莫名其妙回到了入口页 —— 而房间进程其实一直在跑。
 */
let lastView: ConnectView = "entry";

/**
 * 上次挂载时是不是在房间里。
 *
 * 切看 tab 回来时 `connect_state` 是异步的：第一帧还不知道有没有房间，
 * 标题就会先注册成 `null`，等会话查回来才换成「联机房间」——
 * 标题栏那下交叉换位就晚了半拍，看着像"压根没播动画"。
 * 记住上一次的状态，第一帧就按它显示；查回来发现没房间再悄悄改回来
 * （标题栏只认「主页面 ↔ 子页」的切换，同类型改名不会再播一遍）。
 */
let lastHadRoom = false;

/**
 * 联机。
 *
 * 和「模组详情 / 版本设置」一样走全屏子页：入口两张卡 → 点进去整页换掉，
 * 标题栏出现「← 返回」。区别只有一个：切顶部 tab 不会退出会话，
 * 回来还是原来那一页（进度在后端）。
 *
 * 动画：内层 `PageTransition` 负责入口 ↔ 表单 ↔ 房间的切换。
 */
export default function ConnectPage() {
  const [view, setViewState] = useState<ConnectView>(lastView);
  // 第一帧就按上次的状态认房间，别等异步的 connect_state
  const [assumeRoom] = useState(lastHadRoom);
  const setView = useCallback((next: ConnectView) => {
    lastView = next;
    setViewState(next);
  }, []);
  const [status, setStatus] = useState<ConnectModuleStatus | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  const [task, setTask] = useState<InstallTask | null>(null);
  const [starting, setStarting] = useState(false);
  const [session, setSession] = useState<ConnectState | null>(null);
  // 房间名 / 密码从上一次开房记下的值预填，房间因此是「同一个」
  const { config, patch } = useConfig();
  /**
   * 会话状态是否已经问过一次。
   *
   * 不问完就别渲染内容：房间在跑的时候进页面，会先闪一下「创建 / 加入」两张卡，
   * 再被状态卡替换掉 —— 一进一出很像是刚刚有人点了什么。
   */
  const [sessionLoaded, setSessionLoaded] = useState(false);

  /*
   * 联机账号。
   *
   * 没登录就没有「创建 / 加入房间」 —— 节点只放行登录过的客户端，
   * 摆出两个点进去必然失败的入口，比不摆更糟。所以这一层挡在组件下载之前：
   * 连账号都没有，下组件也没用。
   *
   * `loaded` 单独一个标记：登录状态还没问回来时不能先按「未登录」渲染，
   * 否则每次进页面都会闪一下登录卡。
   */
  const [account, setAccount] = useState<ConnectAccount | null>(null);
  const [accountLoaded, setAccountLoaded] = useState(false);
  /**
   * 登录弹窗开在那一档（`null` = 关着）。
   *
   * 未登录时页面上只摆「登录 / 注册」两个按钮，真正的表单在弹窗里 ——
   * 它和这个页面没有耦合（`ConnectLoginDialog` 只认 `open` 和回调），
   * 以后别的页面要登录也能直接引。
   */
  const [authMode, setAuthMode] = useState<ConnectAuthMode | null>(null);

  useEffect(() => {
    void invoke<ConnectAccount>("connect_account_state")
      .then((view) => {
        setAccount(view);
        // 本地文件说「还在登录」不代表服务端还认：令牌可能已经被封号、
        // 在别处退出、或者会话被顶掉。而桥接被拒时一个字节都不回，
        // 玩家只会看到「连不上」，永远不知道是登录没了 —— 所以主动问一次。
        // 网络失败时保持本地判断，不能因为一次超时就把人踢下线。
        if (view.signedIn) {
          void invoke<ConnectAccount>("connect_account_check")
            .then((fresh) => setAccount(fresh))
            .catch(() => {});
        }
      })
      .catch(() => setAccount(null))
      .finally(() => setAccountLoaded(true));
  }, []);

  // 会话状态：进来先取一次（切页回来不丢），之后靠事件推。
  useEffect(() => {
    void invoke<ConnectState>("connect_state")
      .then((view) => setSession(view))
      .catch(() => {})
      .finally(() => setSessionLoaded(true));
    const off = listen<ConnectState>("connect://state", (event) => {
      setSession(event.payload);
    });
    return () => {
      void off.then((fn) => fn());
    };
  }, []);

  const refresh = useCallback(async () => {
    try {
      const next = await invoke<ConnectModuleStatus>("connect_module_status");
      setStatus(next);
      setError(null);
    } catch (err) {
      setError(toAppError(err, "读取联机组件状态失败"));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // 装完 / 装失败：后端会发一条状态事件，刷新即可。下载进度走任务流。
  useEffect(() => {
    const offStatus = listen<ConnectModuleStatus>("connect://module", (event) => {
      setStatus(event.payload);
      if (event.payload.message) {
        setError({
          code: "unknown",
          title: "联机组件没装好",
          message: event.payload.message,
          retryable: true,
        });
      }
    });
    const offProgress = listen<InstallTask>("download://progress", (event) => {
      if (!event.payload.versionId.startsWith(MODULE_TASK_PREFIX)) return;
      setTask(event.payload);
      if (event.payload.phase !== "running") {
        void refresh();
      }
    });
    return () => {
      void offStatus.then((off) => off());
      void offProgress.then((off) => off());
    };
  }, [refresh]);

  const rootRef = useRef<HTMLDivElement>(null);
  const shownLayout = useRef<string | null>(null);

  // 解压这一步没有进度可报。不能只靠事件：事件丢了界面就会一直停在下载态。
  // 所以只要「任务结束了但状态还没就绪」，就每 500ms 主动问一次，直到装好为止。
  const taskFinished = Boolean(task) && task?.phase !== "running";
  useEffect(() => {
    if (status?.ready) return;
    if (!taskFinished && !status?.installing) return;
    const timer = window.setInterval(() => {
      void refresh();
    }, 500);
    return () => window.clearInterval(timer);
  }, [taskFinished, status?.ready, status?.installing, refresh]);
  // 四档布局：转圈 / 登录 / 只有下载入口 / 有侧边栏的内容。
  // 内容晚到时必须自己补一次入场：懒加载把 chunk 拉下来的时候，
  // 外层 `PageTransition` 的入场早就播完了（它当时对着空内容播，0 个块）。
  // 「进房间 / 出房间」不在这里播 —— 那是右栏内层 PageTransition 的事。
  const layout = !accountLoaded
    ? "loading"
    : !account?.signedIn
      ? "login"
      : !status
        ? "loading"
        : status.ready
          ? "ready"
          : "missing";
  useLayoutEffect(() => {
    if (layout === "loading" || shownLayout.current === layout) return;
    shownLayout.current = layout;
    if (rootRef.current) playEnter(rootRef.current);
  }, [layout]);

  // 房间在跑、或者刚失败（要看错误、要能退出重试），都算「在房间里」
  const running = sessionActive(session);
  const inRoom = running || session?.phase === "failed";
  // 会话还没查回来之前，沿用上一次的结论 —— 标题栏才不会晚半拍
  const roomView = inRoom || (assumeRoom && !sessionLoaded);
  useEffect(() => {
    if (sessionLoaded) lastHadRoom = inRoom;
  }, [sessionLoaded, inRoom]);

  /*
   * 标题栏的「← 返回 + 标题」由它接管（同模组详情 / 版本设置）。
   * 点左上角返回 == 点画面上的「退出房间」：在房间里就退出，在表单页就回入口。
   * 必须写在任何 `return` 之前 —— 它是 hook。
   */
  useRegisterNestedPage(
    roomView
      ? "联机房间"
      : view === "entry"
        ? null
        : view === "create"
          ? "创建房间"
          : "加入房间",
    () => {
      if (inRoom) void invoke("connect_stop");
      lastHadRoom = false;
      setView("entry");
    },
  );

  const startInstall = useCallback(async () => {
    setStarting(true);
    setError(null);
    try {
      const initial = await invoke<InstallTask>("connect_module_install");
      setTask(initial);
    } catch (err) {
      setError(toAppError(err, "下载联机组件失败"));
    } finally {
      setStarting(false);
    }
  }, []);

  if (!sessionLoaded || !accountLoaded || (!status && !error)) {
    return (
      <Box ref={rootRef} sx={{ display: "grid", placeItems: "center", minHeight: 240 }}>
        <Loading />
      </Box>
    );
  }

  /*
   * 没登录：页面上只摆「登录账号」和两个按钮，真正的表单在弹窗里。
   *
   * 顺序上登录必须排在「组件没装」前面：节点只放行登录过的客户端，
   * 连账号都没有就先去下 33MB 的组件，等于让人白走一趟。
   */
  if (!account?.signedIn) {
    return (
      <Box
        ref={rootRef}
        sx={{
          flex: 1,
          minHeight: 0,
          minWidth: 0,
          overflowY: "auto",
          scrollbarGutter: "stable",
          p: 2,
          display: "flex",
          flexDirection: "column",
          gap: 1.5,
          "& > *": { flexShrink: 0 },
        }}
      >
        <SettingGroup
          title="登录账号"
          desc="登录之后才能创建和加入房间，好友也得有账号才连得进来。"
        >
          <Box sx={{ px: 2, py: 1.5 }}>
            <Typography sx={{ fontSize: 13, color: "text.secondary", lineHeight: 1.7 }}>
              这是{" "}
              <Box component="span" sx={{ fontWeight: 600, color: "text.primary" }}>
                XCL 自己的账号
              </Box>
              ，只用来连联机节点 —— 和 Minecraft 账号无关，离线、正版、第三方登录都不影响这里。
            </Typography>
            <Stack direction="row" sx={{ gap: 1, mt: 1.5 }}>
              <Button
                variant="contained"
                sx={{ ...BUTTON_SX, flex: 1 }}
                onClick={() => setAuthMode("login")}
              >
                登录
              </Button>
              <Button
                variant="outlined"
                sx={{ ...BUTTON_SX, flex: 1 }}
                onClick={() => setAuthMode("register")}
              >
                注册
              </Button>
            </Stack>
          </Box>
        </SettingGroup>

        <ConnectLoginDialog
          open={authMode !== null}
          initialMode={authMode ?? "login"}
          onClose={() => setAuthMode(null)}
          onSignedIn={(view) => {
            setAccount(view);
            setAuthMode(null);
          }}
        />
      </Box>
    );
  }

  if (!status) {
    return <ErrorState variant="page" error={error ?? undefined} onRetry={refresh} />;
  }

  // 组件没就绪：整页只有下载卡，侧边栏不出现 ——
  // 两个入口都通向同一个下载，摆出来只会让人以为「没有组件也能开房」。
  if (!status.ready) {
    return (
      <Box
        ref={rootRef}
        sx={{
          flex: 1,
          minHeight: 0,
          minWidth: 0,
          overflowY: "auto",
          scrollbarGutter: "stable",
          p: 2,
          display: "flex",
          flexDirection: "column",
          gap: 1.5,
          "& > *": { flexShrink: 0 },
        }}
      >
        <ModuleCard
          status={status}
          task={task}
          busy={starting}
          error={error}
          onInstall={startInstall}
          onRetry={refresh}
        />
      </Box>
    );
  }

  // 房间在跑 / 失败时，页面停在「房间」这一档；否则看停在哪一页
  const current = inRoom ? "room" : view;

  return (
    <Box
      ref={rootRef}
      sx={{
        flex: 1,
        minHeight: 0,
        minWidth: 0,
        display: "flex",
        flexDirection: "column",
        p: 2,
      }}
    >
      <PageTransition
        pageKey={current}
        initial={false}
        render={(key) => (
          // 滚动容器：不标记、不参与动画，只负责滚。
          // 子项必须 `flexShrink: 0`，否则卡片会被压扁到容器高度（同设置页）。
          <Box
            sx={{
              flex: 1,
              minHeight: 0,
              overflow: "auto",
              scrollbarGutter: "stable",
              display: "flex",
              flexDirection: "column",
              gap: 1.5,
              pr: 0.5,
              pb: 1,
              "& > *": { flexShrink: 0 },
            }}
          >
            {key === "room" && session ? (
              <SessionCard state={session} onExit={() => setView("entry")} />
            ) : key === "create" ? (
              <CreateRoomCard
                savedRoom={config?.connect.room ?? ""}
                savedSecret={config?.connect.secret ?? ""}
                // 配置还没读回来时按「看过」算，宁可少弹一次也别闪一下弹窗
                hintSeen={config?.connect.hintSeen ?? true}
                onHintSeen={() => patch({ connect: { hintSeen: true } })}
              />
            ) : key === "join" ? (
              <JoinRoomCard />
            ) : (
              <EntryCards
                onPick={setView}
                account={account}
                onSignedOut={() => {
                  setView("entry");
                  setAccount(null);
                }}
              />
            )}
          </Box>
        )}
      />
    </Box>
  );
}


// 卡片全在 `./connect/` 下：`EntryCards`（入口两张卡）、`ModuleCard`（组件安装）、
// `SessionCard`（当前房间 + 端口 + 设备列表）、`RoomCards`（创建 / 加入表单）、
// `shared`（复制按钮 / 步骤块）。本文件只剩视图切换和轮询。

