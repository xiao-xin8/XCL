import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  Box,
  Button,
  ButtonBase,
  Chip,
  Divider,
  Paper,
  alpha,
  Stack,
  Typography,
} from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import { apiGet, invalidate } from "../lib/api";
import { animationsEnabled } from "../lib/appearance";
import { formatBytes, formatDate } from "../lib/format";
import { useRegisterNestedPage } from "../lib/nestedNav";
import type {
  AppError,
  InstallTarget,
  LocalTarget,
  ModItem,
  ResourceDependency,
  ResourceFile,
  ResourceFileLine,
  ResourceFilesView,
  ResourceGroupsView,
  ResourceKind,
  ResourceProject,
} from "../lib/types";
import { toAppError } from "../lib/types";
import { CONTENT_FADE, DURATION, EASE } from "../motion/tokens";
import { playEnter, playExitAll } from "../motion/pageMotion";
import PageTransition from "../motion/PageTransition";
import {
  isFilePairDone,
  peekTaskForFile,
  useFileDoneSignal,
  useTaskForFile,
} from "../lib/downloadStore";
import { TaskProgress } from "./DownloadTask";
import ErrorState from "./ErrorState";
import Loading from "./Loading";
import PageLoaderGate from "./PageLoaderGate";
import ResourceProjectPanel from "./ResourceProjectPanel";
import SlidePanel from "./SlidePanel";
import CollapsibleSection from "./CollapsibleSection";
import { DependencyBlock } from "./resource-detail/DependencyBlock";
import {
  FileRow,
  InstallTargetRow,
  LocalTargetRow,
  ManualFileRowList,
} from "./resource-detail/rows";
import {
  INSTALL_HINT,
  NUM,
  PANEL_W,
  RESOURCE_NOUN,
  ROW_PAGE,
  loaderLabel,
  panelListSx,
} from "./resource-detail/shared";
import type { TargetRef } from "./resource-detail/shared";

// markdown 渲染栈已经打进主包，不再单独切 chunk —— 详情页那一条必须秒出。
// 取舍见 `docs/PERFORMANCE.md`。

/**
 * 资源详情页。
 *
 * 两套流程，不是同一个东西的两档：
 * - 自动模式：左侧是「本地版本被这个 Mod 割过一刀」剩下的那些，一行一个本地版本。
 *   点一行 → 右侧升入，列该本地版本能用的全部文件版本（不预选、不做下拉）。
 * - 手动模式：左侧是这个 Mod 的全部文件版本，不做任何兼容判断。
 *   点一条 → 右侧升入，列出本地所有能装 Mod 的版本，选一个等于选了那个 `mods/`；
 *   不想进版本就点「自定义文件夹」。
 *
 * 两侧各定一个维度，进去不替选任何东西。
 *
 * `kind` 决定文案、落点（`mods/` / `resourcepacks/` / `shaderpacks/`）和候选版本
 * 的筛选规则 —— 三种类型共用这一页，不是只有 Mod。
 *
 * 子块都在 `./resource-detail/` 下：`shared.ts`（常量 / 文案 / 样式片段）、
 * `rows.tsx`（各种列表行）、`DependencyBlock.tsx`（依赖区块）。
 * 本文件只留状态机 + 两栏布局，加东西前先想想该不该放进子块。
 */

export default function ResourceDetailPage(props: {
  item: ModItem;
  /**
   * 下载完成时通知上一层。
   *
   * 根详情页（从 Mod 列表进来）不传：下载完成后停在原地看「已安装」，
   * 不再自动退回列表。依赖页传「关掉自己」—— 下完自动回到跳转前的 Mod。
   */
  onDone?: () => void;
  /** 从依赖跳转过来时，自动选中跳转前那个本地版本 */
  initialTarget?: TargetRef;
  /** 依赖页沿用的模式：自动 / 手动 */
  initialMode?: "auto" | "manual";
  /** 已经走过的项目链（防循环依赖：A → B → A 不再跳） */
  visitedProjects?: string[];
  /**
   * 资源类型。搜索 / 文件列表 / 落点全都按它走，界面只有文案上的差别：
   * 资源包不看加载器、没有依赖、落到 `resourcepacks/`。
   */
  kind?: ResourceKind;
}) {
  const {
    item,
    onDone,
    initialTarget,
    initialMode = "auto",
    visitedProjects,
    kind = "mod",
  } = props;

  /** 文案里的名词：`Mod` / `资源包` */
  const noun = RESOURCE_NOUN[kind];

  /** 当前页所在的项目链（根页面只有自己） */
  const projectChain = [...(visitedProjects ?? []), item.id];

  const [mode, setMode] = useState<"auto" | "manual">(initialMode);

  /** 自动模式：左侧本地版本。`null` = 还在加载 */
  const [targets, setTargets] = useState<LocalTarget[] | null>(null);
  const [targetsError, setTargetsError] = useState<AppError | null>(null);
  const [activeVersion, setActiveVersion] = useState<string | null>(null);
  /** 每次切换模式 +1：保证「空列表 → 手动 → 空 → 手动」第二次进入仍播入场 */
  const [modeEpoch, setModeEpoch] = useState(0);

  /** 自动模式：右栏文件列表 */
  const [files, setFiles] = useState<ResourceFile[]>([]);
  const [filesLoading, setFilesLoading] = useState(false);
  const [activeFile, setActiveFile] = useState<string | null>(null);

  /** 右栏依赖区块 */
  const [deps, setDeps] = useState<ResourceDependency[] | null>(null);
  const [depsError, setDepsError] = useState<AppError | null>(null);
  const [depsReload, setDepsReload] = useState(0);
  /** 当前 `deps` 属于哪一对「文件@落点」。自动模式依赖会跟文件列表一起回来，
   *  这里记下键，避免又发一次同样的请求。 */
  const depsKeyRef = useRef<string | null>(null);

  /** 左列「项目详情」卡 */
  const [project, setProject] = useState<ResourceProject | null>(null);
  const [projectLoading, setProjectLoading] = useState(true);
  const [projectError, setProjectError] = useState<AppError | null>(null);
  const [projectReload, setProjectReload] = useState(0);

  /**
   * 手动模式：左侧全量文件，三级折叠（版本线 → 具体版本 → 文件行），
   * 和「下载 → 游戏」的版本树同构。右栏是本地版本。
   *
   * 为什么必须分三级、而且默认全收起：实测 JEI 有 3986 个文件，
   * 最新的 `26.3` 一个版本就挂着 1844 个文件。平铺一层 = 进页面立刻
   * 构造近两千行、上万 DOM 节点，之后每一次点击、每一次开关侧栏都要
   * 重排那么高的内容 —— 整页"点什么都要等一下"就是这么来的。
   */
  const [manualLines, setManualLines] = useState<ResourceFileLine[] | null>(null);
  const [openLines, setOpenLines] = useState<Set<string>>(new Set());
  const [openVersions, setOpenVersions] = useState<Set<string>>(new Set());
  /** 单个版本组「已经放出来多少行」；一个版本挂上千个文件时靠它分页 */
  const [shownRows, setShownRows] = useState<Record<string, number>>({});
  const [manualFile, setManualFile] = useState<string | null>(null);
  const [installTargets, setInstallTargets] = useState<InstallTarget[]>([]);
  /** 手动模式左侧的平台筛选（`all` = 不筛）。选项来自真正拉回来的文件列表 */
  const [manualLoader, setManualLoader] = useState("all");
  /** 平台胶囊的滑动指示块：位置 / 宽度跟着选中的按钮走 */
  const [loaderPill, setLoaderPill] = useState({ left: 0, width: 0, ready: false });
  const pillRefs = useRef<Record<string, HTMLButtonElement | null>>({});
  const pillStripRef = useRef<HTMLDivElement>(null);
  /** 依赖页进来时沿用同一个本地版本（手动模式下先选中，等点文件再升栏） */
  const [manualTarget, setManualTarget] = useState<string | null>(
    initialMode === "manual" ? (initialTarget?.versionId ?? null) : null,
  );

  const [installError, setInstallError] = useState<AppError | null>(null);
  /** 正在发起的那一对「文件@落点」（不按整页算，否则一个在点会把其它行也锁住） */
  const [startingKey, setStartingKey] = useState<string | null>(null);
  /** 手动模式「自定义文件夹」选过的目录。选了它 = 落点不再是一个本地实例 */
  const [customDir, setCustomDir] = useState<string | null>(null);

  /**
   * 依赖页：全屏切到依赖 Mod 自己的详情页（同一个组件递归）。
   *
   * `lastChild` 保留一份"正在退场的那一页"：点返回时 `child` 立刻变 null，
   * 但旧页还要播 110ms 退场，`PageTransition.render` 可能还在要它。
   */
  const [child, setChild] = useState<{
    item: ModItem;
    /** 手动模式还没选安装位置时为空：依赖页正常打开，不自动选版本 */
    target?: TargetRef;
    mode: "auto" | "manual";
    visited: string[];
  } | null>(null);
  const lastChild = useRef<typeof child>(null);
  if (child) lastChild.current = child;

  /** 自动模式进来后一次性选中跳转前的那个本地版本（找不到就不强选） */
  const appliedInitial = useRef(false);

  const listRef = useRef<HTMLDivElement>(null);

  /**
   * 依赖页的标题栏：进入依赖页时把栈顶换成依赖 Mod，关掉后自动落回本页。
   * 根页面的注册由 ModPage 负责（那里点下去标题就换，不等 110ms）。
   */
  useRegisterNestedPage(child ? child.item.title : null, () => setChild(null));

  // ---------- 数据 ----------

  /**
   * `silent = true`：只刷新「已安装」标记，不把列表打回转圈。
   * 下载完成时会用，正看着列表，不能闪一下再回来。
   */
  const loadTargets = useCallback(
    (silent = false) => {
      if (!silent) setTargets(null);
      setTargetsError(null);
      apiGet<LocalTarget[]>("resource_targets", {
        source: item.source,
        id: item.id,
        kind,
      })
        .then((list) => setTargets(list))
        .catch((error: unknown) => {
          // 静默刷新失败就保持原样，别把已经看到的列表清空
          if (silent) return;
          setTargets([]);
          setTargetsError(toAppError(error, "读取本地版本失败"));
        });
    },
    [item.id, item.source, kind],
  );

  useEffect(() => {
    loadTargets();
  }, [loadTargets]);

  // 项目详情：进页面就拉一次（前端有 5 分钟缓存），折叠行先用列表里的摘要。
  useEffect(() => {
    let alive = true;
    setProjectLoading(true);
    setProjectError(null);
    apiGet<ResourceProject>("resource_project", {
      source: item.source,
      id: item.id,
      kind,
    })
      .then((view) => {
        if (alive) setProject(view);
      })
      .catch((error: unknown) => {
        if (!alive) return;
        setProjectError(toAppError(error, "读取项目详情失败"));
      })
      .finally(() => {
        if (alive) setProjectLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [item.source, item.id, projectReload, kind]);

  // 自动模式：点一行才去拿那个本地版本的文件
  useEffect(() => {
    if (mode !== "auto" || !activeVersion) return;
    let alive = true;
    // 先清空：否则点另一行时右栏会短暂显示上一个版本的文件，而下载按钮是亮的
    // —— 那一下点下去会把错的文件塞进新版本（后端不做二次校验）。
    setFiles([]);
    setActiveFile(null);
    setDeps(null);
    setDepsError(null);
    depsKeyRef.current = null;
    setFilesLoading(true);
    setInstallError(null);
    apiGet<ResourceFilesView>("resource_files", {
      source: item.source,
      id: item.id,
      versionId: activeVersion,
      kind,
    })
      .then((view) => {
        if (!alive) return;
        setFiles(view.items);
        setActiveFile(view.defaultFileId);
        // 依赖跟列表一起回来的：记下键，防止依赖 effect 又发一次同样的请求
        if (view.defaultFileId) {
          depsKeyRef.current = `${view.defaultFileId}@${activeVersion}`;
          setDeps(view.dependencies);
        }
      })
      .catch((error: unknown) => {
        if (!alive) return;
        setFiles([]);
        setActiveFile(null);
        setInstallError(toAppError(error, "读取文件列表失败"));
      })
      .finally(() => {
        if (alive) setFilesLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [activeVersion, item.id, item.source, mode, kind]);

  // 手动模式：第一次进来才拉全量（三级分组）+ 本地版本列表
  useEffect(() => {
    if (mode !== "manual" || manualLines !== null) return;
    let alive = true;
    setFilesLoading(true);
    setInstallError(null);
    Promise.all([
      apiGet<ResourceGroupsView>("resource_file_groups", {
        source: item.source,
        id: item.id,
      }),
      invoke<InstallTarget[]>("resource_install_targets", { kind }),
    ])
      .then(([view, versions]) => {
        if (!alive) return;
        setManualLines(view.lines);
        // 默认全部收起（和版本树一样）：进来只挂几条版本线，
        // 展开哪一组才构造哪一组的行。
        setOpenLines(new Set());
        setOpenVersions(new Set());
        setShownRows({});
        setInstallTargets(versions);
      })
      .catch((error: unknown) => {
        if (!alive) return;
        setManualLines([]);
        setInstallError(toAppError(error, "读取文件列表失败"));
      })
      .finally(() => {
        if (alive) setFilesLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [mode, manualLines, item.id, item.source, kind]);

  const toggleIn = useCallback(
    (setter: React.Dispatch<React.SetStateAction<Set<string>>>, key: string) => {
      setter((prev) => {
        const next = new Set(prev);
        if (next.has(key)) next.delete(key);
        else next.add(key);
        return next;
      });
    },
    [],
  );

  /** 「显示更多」：给这个版本组再放 100 行出来。 */
  const showMore = useCallback((key: string) => {
    setShownRows((prev) => ({ ...prev, [key]: (prev[key] ?? ROW_PAGE) + ROW_PAGE }));
  }, []);

  // ---------- 下载 ----------

  /**
   * 当前选中的「文件 + 落点」。
   *
   * 下载状态按这一对算，而不是「本页最后发起的那一条任务」：
   * 同一个 Mod 的每个文件版本、每个目标实例各自是独立的进度和按钮状态，
   * 一条在跑不会把其它行一起锁住。
   */
  const targetId =
    mode === "auto" ? activeVersion : (manualTarget ?? customDir);
  const pairFile = mode === "auto" ? activeFile : manualFile;
  const pairKey = pairFile && targetId ? `${pairFile}@${targetId}` : null;

  const task = useTaskForFile(pairFile, targetId);
  const running = task?.phase === "running";
  const starting = startingKey !== null && startingKey === pairKey;

  /**
   * 当前落点对应的本地版本信息（依赖解析要 MC 版本 + 加载器）。
   * 自定义文件夹没有版本信息，所以这条路径永远没有依赖。
   */
  const targetRef: TargetRef | null =
    mode === "auto"
      ? (() => {
          const found = (targets ?? []).find(
            (target) => target.versionId === activeVersion,
          );
          return found
            ? {
                versionId: found.versionId,
                gameVersion: found.gameVersion,
                loader: found.loader,
              }
            : null;
        })()
      : (() => {
          const found = installTargets.find(
            (target) => target.versionId === manualTarget,
          );
          return found
            ? {
                versionId: found.versionId,
                gameVersion: found.gameVersion,
                loader: found.loader,
              }
            : null;
        })();
  const targetVersionId = targetRef?.versionId ?? null;

  // 依赖页：本地版本列表回来后，自动选中跳转前那一行（并让右栏升入）
  useEffect(() => {
    if (appliedInitial.current || !initialTarget) return;
    if (mode !== "auto" || targets === null) return;
    appliedInitial.current = true;
    if (targets.some((target) => target.versionId === initialTarget.versionId)) {
      setActiveVersion(initialTarget.versionId);
    }
  }, [initialTarget, mode, targets]);

  // 依赖区块：只要选中了文件就查（手动模式不需要先选安装位置）。
  // 传了本地版本才会补「具体依赖文件 + 已安装」。
  useEffect(() => {
    if (!pairFile) {
      depsKeyRef.current = null;
      setDeps(null);
      setDepsError(null);
      return;
    }
    const key = `${pairFile}@${targetVersionId ?? "none"}`;
    // 自动模式的依赖已经跟文件列表一起回来了，别重复请求
    if (depsKeyRef.current === key) return;
    let alive = true;
    setDeps(null);
    setDepsError(null);
    apiGet<ResourceDependency[]>("resource_dependencies", {
      source: item.source,
      id: item.id,
      fileId: pairFile,
      targetVersionId,
      kind,
    })
      .then((list) => {
        if (!alive) return;
        depsKeyRef.current = key;
        setDeps(list);
      })
      .catch((error: unknown) => {
        if (!alive) return;
        depsKeyRef.current = key;
        setDeps([]);
        setDepsError(toAppError(error, "读取依赖失败"));
      });
    return () => {
      alive = false;
    };
  }, [pairFile, targetVersionId, item.id, item.source, depsReload, kind]);

  // 任务从「发起到出现在 store 里」有几十毫秒空窗：这段时间不能让按钮
  // 从「已开始」闪回「下载」，否则会以为没点上又点一次。
  useEffect(() => {
    if (task) setStartingKey(null);
  }, [task]);

  /**
   * 下完就退回列表 —— 但只认这次进页面自己发起的下载。
   *
   * 现在可以连着下好几个：完成一条就往回跳，会把人从还没下完的队列里甩出来；
   * 而别的页面 / 别的 Mod 下完，更不该把人从这里踢走。两个条件都满足才退回：
   * 自己发起的有下完的，且自己发起的已经都不在跑了。
   */
  const fileDone = useFileDoneSignal();
  const seenFileDone = useRef(fileDone);
  const startedPairs = useRef<{ fileId: string; target: string }[]>([]);

  useEffect(() => {
    if (fileDone === seenFileDone.current) return;
    seenFileDone.current = fileDone;
    // 下次进来要看到「已安装」，不能让 5 分钟的前端缓存把结论盖住
    invalidate("resource_targets");
    invalidate("resource_files");
    invalidate("resource_dependencies");
    invalidate("version_stats");
    loadTargets(true);
    // 依赖页下完回到这里时，依赖卡也要变「已安装」
    depsKeyRef.current = null;
    setDepsReload((n) => n + 1);

    const mine = startedPairs.current;
    const anyDone = mine.some((pair) => isFilePairDone(pair.fileId, pair.target));
    const stillRunning = mine.some(
      (pair) => peekTaskForFile(pair.fileId, pair.target)?.phase === "running",
    );
    // 只通知上一层：依赖页下完自动回原 Mod；根页面没有上一层，就停在原地
    if (anyDone && !stillRunning) onDone?.();
  }, [fileDone, loadTargets, onDone]);

  const startDownload = useCallback(
    async (fileId: string, target: string, targetDir?: string) => {
      setStartingKey(`${fileId}@${target}`);
      setInstallError(null);
      // 先记下来再发起：很小的文件可能比命令返回还早下完，
      // 完成信号先到时就认不出「这是我自己发的」了
      const mine = { fileId, target };
      startedPairs.current = [...startedPairs.current, mine];
      try {
        await invoke("resource_install", {
          source: item.source,
          id: item.id,
          fileId,
          targetVersionId: targetDir ? null : target,
          targetDir: targetDir ?? null,
          kind,
        });
        // 这里不清 starting：等 store 收到这条任务的进度再清（见上面的 effect）
      } catch (error: unknown) {
        startedPairs.current = startedPairs.current.filter((pair) => pair !== mine);
        setStartingKey(null);
        setInstallError(toAppError(error, "开始下载失败"));
      }
    },
    [item.id, item.source, kind],
  );

  const pickFolderAndDownload = useCallback(async () => {
    if (!manualFile) return;
    try {
      const dir = await invoke<string | null>("pick_folder");
      if (!dir) return;
      // 选自定义文件夹 = 换落点：清掉右栏选中的实例，进度按这个目录算
      setManualTarget(null);
      setCustomDir(dir);
      await startDownload(manualFile, dir, dir);
    } catch (error: unknown) {
      setInstallError(toAppError(error, "选择文件夹失败"));
    }
  }, [manualFile, startDownload]);

  // ---------- 模式切换 ----------

  const switchMode = useCallback(
    (next: "auto" | "manual") => {
      if (next === mode) return;
      setInstallError(null);
      setStartingKey(null);
      setCustomDir(null);
      // 旧列表先整体淡出（不是逐条错峰），再换内容 —— 换的是内容不是页面
      const root = listRef.current;
      if (!root) {
        setMode(next);
        setModeEpoch((n) => n + 1);
        return;
      }
      playExitAll(root);
      window.setTimeout(() => {
        setMode(next);
        // 换内容也算一次新进入：下一次回到这个模式必须重新播动画
        setModeEpoch((n) => n + 1);
      }, DURATION.exit);
    },
    [mode],
  );

  // ---------- 列表动画 ----------

  /**
   * 手动模式的平台选项：从实际拉回来的文件列表里收集，不写死。
   * 顺序固定，方便形成肌肉记忆。
   */
  const platformOptions = useMemo(() => {
    const found = new Set<string>();
    for (const line of manualLines ?? []) {
      for (const version of line.versions) {
        for (const file of version.items) {
          for (const loader of file.loaders) found.add(loader.toLowerCase());
        }
      }
    }
    return ["fabric", "forge", "neoforge", "quilt"].filter((loader) => found.has(loader));
  }, [manualLines]);

  /** 按平台过滤后的三级树；空版本 / 空版本线不留下，count 重新算 */
  const manualLinesShown = useMemo(() => {
    if (!manualLines || manualLoader === "all") return manualLines;
    return manualLines
      .map((line) => {
        const versions = line.versions
          .map((version) => ({
            ...version,
            items: version.items.filter((file) =>
              file.loaders.some((loader) => loader.toLowerCase() === manualLoader),
            ),
          }))
          .filter((version) => version.items.length > 0);
        const count = versions.reduce((sum, version) => sum + version.items.length, 0);
        return { ...line, versions, count };
      })
      .filter((line) => line.versions.length > 0);
  }, [manualLines, manualLoader]);

  const selectPlatform = useCallback(
    (next: string) => {
      if (next === manualLoader) return;
      setManualLoader(next);
      setInstallError(null);
      setStartingKey(null);
      // 选中的文件不在新平台里就收起侧栏，避免侧栏开着一个看不见的文件
      if (manualFile && next !== "all") {
        const visible = (manualLines ?? []).some((line) =>
          line.versions.some((version) =>
            version.items.some(
              (file) =>
                file.fileId === manualFile &&
                file.loaders.some((loader) => loader.toLowerCase() === next),
            ),
          ),
        );
        if (!visible) setManualFile(null);
      }
    },
    [manualLoader, manualFile, manualLines],
  );

  /**
   * 平台胶囊的滑动块：测量选中按钮的位置，画在按钮下面。
   * 位置 / 宽度变化用 CSS transition（和 MUI 的 Tab 指示块同一套思路），
   * 时长复用现成的 `DURATION.enterMoveLeft` + `EASE.outFluent`，不新造参数。
   */
  useLayoutEffect(() => {
    const button = pillRefs.current[manualLoader];
    if (!button) {
      setLoaderPill((prev) => ({ ...prev, ready: true }));
      return;
    }
    setLoaderPill({
      left: button.offsetLeft,
      width: button.offsetWidth,
      ready: true,
    });
  }, [manualLoader, platformOptions.length, manualLines]);

  // 选中的平台在重新拉数据后不存在了（例如切换 Mod）→ 回到「全部」
  useEffect(() => {
    if (manualLoader !== "all" && !platformOptions.includes(manualLoader)) {
      setManualLoader("all");
    }
  }, [manualLoader, platformOptions]);

  const rows = mode === "auto" ? targets : manualLinesShown;
  const loadingList = rows === null;
  /**
   * `modeEpoch` 参与 key：空列表不播入场时 `lastAnim` 不会更新，
   * 旧的 `${mode}:${n}` 会让第二次进入手动模式被误判成"播过了"。
   * 每次切换模式 +1，第二次进入就是新 key。空列表依旧不播（`pendingAnim` 为空）。
   * 平台筛选也算一次「换了列表」，同样要重播。
   */
  const animKey = `${mode}:${modeEpoch}:${manualLoader}:${rows === null ? "loading" : rows.length}`;
  /** 上一次真正播过入场的 key */
  const lastAnim = useRef("");
  /** 「该播但还没播」的 key：列表节点还没挂载时先记在这里 */
  const pendingAnim = useRef("");

  /**
   * 补播欠着的入场动画。
   *
   * 关键在没有走缓存的那条路：数据回来时 `loadingList` 已经 false，但
   * `PageLoaderGate` 的环还在补它那 400ms 最短停留，列表节点还没挂载。
   * 这时候 effect 里既拿不到 root，又已经把 key 记成"播过了" —— 于是
   * 转圈消失后列表是硬生生出现的，一点动画都没有（命中缓存则不会经过
   * 转圈，节点一直在，所以看起来"有动画"）。
   *
   * 所以：key 只记到 `pendingAnim`，真正播过才写进 `lastAnim`；
   * 节点挂载（ref 回调）时再补一次。
   */
  const playPending = useCallback(() => {
    const root = listRef.current;
    const key = pendingAnim.current;
    if (!root || !key || key === lastAnim.current) return;
    lastAnim.current = key;
    playEnter(root);
  }, []);

  useLayoutEffect(() => {
    pendingAnim.current = rows && rows.length > 0 ? animKey : "";
    playPending();
  }, [animKey, rows, playPending]);

  /** 稳定的 ref 回调：节点真正挂上的那一刻补播欠着的动画 */
  const attachList = useCallback(
    (node: HTMLDivElement | null) => {
      listRef.current = node;
      if (node) playPending();
    },
    [playPending],
  );

  // ---------- 渲染 ----------

  /**
   * 手动模式的文件索引。
   *
   * 以前每次渲染都 `flatMap` 一遍：一个老 Mod 有四千多条目（含各版本重复），
   * 每点一下都要重新摊平一次，是"点什么都要等一下"的一部分。
   * 这里只在线组数据变化时建一次 Map，之后是 O(1) 查。
   */
  const manualFileMap = useMemo(() => {
    const map = new Map<string, ResourceFile>();
    for (const line of manualLines ?? []) {
      for (const version of line.versions) {
        for (const file of version.items) map.set(file.fileId, file);
      }
    }
    return map;
  }, [manualLines]);

  const selectedFile =
    mode === "auto"
      ? files.find((file) => file.fileId === activeFile) ?? null
      : manualFile
        ? (manualFileMap.get(manualFile) ?? null)
        : null;

  const panelOpen = mode === "auto" ? activeVersion !== null : manualFile !== null;

  /**
   * 右栏正文「转圈 → 内容」那一刻的淡入。
   *
   * 说的是这里的淡入，不是面板开关：右侧栏打开后先转圈（拉文件列表），
   * 加载完内容一下"啪"地出现，中间没有任何过渡。这里只补一段纯 opacity
   * 淡入（200ms，跟 `SlidePanel` 同一档），不做位移。
   *
   * key 用「列表内容」而不是「loading 变 false」：切本地版本、切模式都会重新
   * 走一遍，而同一份内容重渲染不会重复播。
   */
  const panelBodyRef = useRef<HTMLDivElement>(null);
  const panelListKey =
    mode === "auto"
      ? filesLoading
        ? "loading"
        : files.length > 0
          ? `files:${activeVersion ?? ""}:${files.length}`
          : "empty"
      : installTargets.length > 0
        ? `targets:${installTargets.length}`
        : "empty";
  const lastPanelKey = useRef("");

  useLayoutEffect(() => {
    if (panelListKey === lastPanelKey.current) return;
    lastPanelKey.current = panelListKey;
    if (panelListKey === "loading" || panelListKey === "empty") return;
    const el = panelBodyRef.current;
    if (!el || !animationsEnabled()) return;

    const fade = el.animate([{ opacity: 0 }, { opacity: 1 }], {
      duration: CONTENT_FADE.duration,
      easing: CONTENT_FADE.easing,
      fill: "both",
    });
    fade.finished.then(() => fade.cancel()).catch(() => undefined);
  }, [panelListKey]);

  /** 手动模式的「自定义文件夹」：没有版本信息，不解析依赖，直接下单文件 */
  const isCustomDir = mode === "manual" && customDir !== null;

  /** 文件本身已经告诉我们「有没有依赖」：首屏回来就能决定要不要先出加载动画 */
  const hasDeps = (selectedFile?.dependencyCount ?? 0) > 0;
  /** 只展示「属于当前这一对文件@落点」的依赖，避免切文件时闪一下上一份 */
  const pairKeyForDeps =
    pairFile && targetVersionId ? `${pairFile}@${targetVersionId}` : null;
  const depsShown =
    pairKeyForDeps && depsKeyRef.current === pairKeyForDeps ? deps : null;
  const depsErrorShown =
    pairKeyForDeps && depsKeyRef.current === pairKeyForDeps ? depsError : null;
  const depPending =
    !isCustomDir &&
    hasDeps &&
    depsShown === null &&
    depsErrorShown === null;
  const showDepBlock =
    !isCustomDir &&
    (depsErrorShown !== null ||
      depPending ||
      (depsShown !== null && depsShown.length > 0));

  /**
   * 依赖只是善意提醒：没装依赖也允许装当前 Mod（可能自己处理过），
   * 所以这里不再拿依赖状态拦下载按钮。
   */


  /** 右栏标题下面那行：右栏现在是给哪个本地版本选文件 */
  const activeTarget =
    (targets ?? []).find((target) => target.versionId === activeVersion) ?? null;

  const canDownload =
    mode === "auto"
      ? Boolean(activeFile) &&
        Boolean(activeVersion) &&
        // 已经装过这个 Mod 的本地版本：还能点开看文件/依赖，但按钮显示「已安装」
        !activeTarget?.installed &&
        !starting &&
        !running &&
        !filesLoading
      : Boolean(manualFile) &&
        Boolean(manualTarget ?? customDir) &&
        !starting &&
        !running;

  const installLabel =
    mode === "auto" && activeTarget?.installed
      ? "已安装"
      : running || starting
        ? "已开始"
        : "下载";

  /**
   * 装完提示：文案来自 `INSTALL_HINT`，只有「装完还得去游戏里启用一次」的类型才有。
   * 触发时机分两种 —— 自动模式看后端给的 `installed`，手动模式看这一对
   * 「文件@落点」本轮是不是刚下完。
   */
  const hintTarget = manualTarget ?? customDir;
  const installHint = INSTALL_HINT[kind];
  const showInstallHint =
    Boolean(installHint) &&
    (installLabel === "已安装" ||
      (mode === "manual" && isFilePairDone(manualFile, hintTarget)));

  const install = () => {
    // 自定义文件夹：只要文件，不碰依赖
    if (isCustomDir) {
      if (manualFile && customDir) void startDownload(manualFile, customDir, customDir);
      return;
    }
    if (mode === "auto") {
      if (activeFile && activeVersion) void startDownload(activeFile, activeVersion);
      return;
    }
    if (manualFile && manualTarget) void startDownload(manualFile, manualTarget);
  };

  /** 依赖卡片上的「前往安装」：跳到那个依赖 Mod 的详情页（同一组件递归） */
  const openDependency = (dep: ResourceDependency) => {
    setChild({
      item: dep.project,
      target: targetRef ?? undefined,
      mode,
      visited: projectChain,
    });
  };

  const detail = (
    <Box sx={{ flex: 1, minWidth: 0, minHeight: 0, display: "flex" }}>
      {/* 左：本地版本（自动）/ 全部文件版本（手动） */}
      <Box sx={{ flex: 1, minWidth: 0, minHeight: 0, display: "flex", flexDirection: "column" }}>
        {/* 项目详情：放在最顶上（连平台筛选 Tab 也在它下面） */}
        <ResourceProjectPanel
          item={item}
          project={project}
          loading={projectLoading}
          error={projectError}
          onRetry={() => {
            invalidate("resource_project");
            setProjectReload((n) => n + 1);
          }}
        />

        {/* 操作行：自动模式给「手动安装」入口，手动模式给说明 + 退出 */}
        <Stack
          direction="row"
          data-page-block
          spacing={1}
          sx={{ alignItems: "center", flexShrink: 0, mb: 1, minWidth: 0, minHeight: 36 }}
        >
          {mode === "manual" ? (
            <>
              {/* 平台筛选和「退出」同一行；胶囊样式对齐 Mod 列表页的源切换。
                  `platformOptions` 只在拉回来的文件里真的出现 fabric/forge/… 时才有值：
                  资源包的文件不带这些，这里就只剩一个「全部平台」——
                  那是个永远筛不出东西的按钮，干脆整条不渲染。 */}
              {manualLines !== null && platformOptions.length > 0 && (
                <Box
                  sx={{
                    flexShrink: 1,
                    minWidth: 0,
                    maxWidth: "calc(100% - 48px)",
                    height: 36,
                    p: "3px",
                    borderRadius: 999,
                    bgcolor: (t) =>
                      alpha(t.palette.text.primary, 0.06 * t.palette.surface.opacity),
                    backdropFilter: (t) => t.palette.surface.filter,
                    display: "flex",
                    alignItems: "center",
                    // 胶囊宽度跟着按钮走；平台多了超出可用宽度就横向滚动
                    overflowX: "auto",
                    overflowY: "hidden",
                    scrollbarWidth: "none",
                    "&::-webkit-scrollbar": { display: "none" },
                  }}
                >
                <Box
                  ref={pillStripRef}
                  sx={{
                    position: "relative",
                    display: "flex",
                    alignItems: "center",
                    width: "max-content",
                    flexShrink: 0,
                  }}
                >
                  {/* 金色滑动块：位置 / 宽度跟着选中的按钮走 */}
                  <Box
                    aria-hidden
                    sx={{
                      position: "absolute",
                      top: 0,
                      left: loaderPill.left,
                      width: loaderPill.width,
                      height: 30,
                      borderRadius: 999,
                      bgcolor: "primary.main",
                      // 淡黄和背景太近，靠一圈深金细边分出来（和源切换一致）
                      boxShadow: (t) =>
                        `inset 0 0 0 1px ${alpha(t.palette.brand.ink, 0.22)}`,
                      pointerEvents: "none",
                      transition: loaderPill.ready
                        ? `left ${DURATION.enterMoveLeft}ms ${EASE.outFluent}, width ${DURATION.enterMoveLeft}ms ${EASE.outFluent}`
                        : "none",
                    }}
                  />
                  {["all", ...platformOptions].map((loader) => {
                    const selected = manualLoader === loader;
                    return (
                      <ButtonBase
                        key={loader}
                        ref={(node) => {
                          pillRefs.current[loader] = node;
                        }}
                        disableRipple
                        onClick={() => selectPlatform(loader)}
                        sx={{
                          position: "relative",
                          zIndex: 1,
                          height: 30,
                          minHeight: 30,
                          flexShrink: 0,
                          px: 1.5,
                          borderRadius: 999,
                          fontSize: 12.5,
                          fontWeight: 500,
                          whiteSpace: "nowrap",
                          color: selected ? "primary.contrastText" : "text.secondary",
                          bgcolor: "transparent",
                          transition: "color 160ms ease",
                          "&:hover": selected
                            ? undefined
                            : {
                                color: "text.primary",
                                bgcolor: (t) => alpha(t.palette.text.primary, 0.06),
                              },
                        }}
                      >
                        {loader === "all" ? "全部" : loaderLabel(loader)}
                      </ButtonBase>
                    );
                  })}
                </Box>
                </Box>
              )}
              <Box sx={{ flex: 1 }} />
              <Button
                size="small"
                onClick={() => switchMode("auto")}
                sx={{ minWidth: 0, px: 1, flexShrink: 0 }}
              >
                退出
              </Button>
            </>
          ) : (
            <>
              <Typography sx={{ fontSize: 13.5, color: "text.secondary", minWidth: 0 }} noWrap>
                选择您要安装{noun}的本地版本
              </Typography>
              <Box sx={{ flex: 1 }} />
              {/* 尺寸走主题的 small，hover 交给主题 —— 不要在这里覆盖 */}
              <Button
                size="small"
                variant="outlined"
                onClick={() => switchMode("manual")}
              >
                手动安装
              </Button>
            </>
          )}
        </Stack>

        {targetsError && mode === "auto" && (
          <ErrorState
            variant="inline"
            error={targetsError}
            onRetry={() => loadTargets()}
            sx={{ mb: 1, flexShrink: 0 }}
          />
        )}

        {/* 卡片流：一条一个 Paper，和 Mod 列表同一套（逐条落下） */}
        <Box sx={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>
          <PageLoaderGate loading={loadingList}>
            <Box
              ref={attachList}
              data-page-stagger
              sx={{
                flex: 1,
                minHeight: 0,
                overflowY: "auto",
                scrollbarGutter: "stable",
                display: "flex",
                flexDirection: "column",
                gap: 1,
                pb: 1,
              }}
            >
              {mode === "auto"
                ? (targets ?? []).map((target) => (
                    <LocalTargetRow
                      key={target.versionId}
                      target={target}
                      active={activeVersion === target.versionId}
                      onClick={() => {
                        // 已安装的版本也能点开：要看文件/依赖，不能因为装过就锁死
                        setActiveVersion(target.versionId);
                      }}
                    />
                  ))
                : (manualLinesShown ?? []).length === 0 ? null : (
                    (manualLinesShown ?? []).map((line) => (
                      <Paper
                        key={line.label}
                        variant="outlined"
                        sx={{
                          borderRadius: 1,
                          overflow: "hidden",
                          // 必须：flex 子项 + overflow:hidden 时自动最小高度是 0，
                          // 会被压到容器高度、内容裁掉，外层就再也滚不动了
                          flexShrink: 0,
                        }}
                      >
                        {/* 第一级：版本线（1.21 / 1.20 / 预览版） */}
                        <CollapsibleSection
                          level={1}
                          open={openLines.has(line.label)}
                          onToggle={() => toggleIn(setOpenLines, line.label)}
                          count={line.count}
                          header={
                            <Typography sx={{ fontSize: 14, fontWeight: 700 }}>
                              {line.label}
                            </Typography>
                          }
                          renderBody={() =>
                            line.versions.map((version) => {
                              const key = `${line.label}:${version.label}`;
                              const shown = shownRows[key] ?? ROW_PAGE;

                              return (
                                // 第二级：具体 MC 版本（1.21.11）
                                <CollapsibleSection
                                  key={key}
                                  level={2}
                                  open={openVersions.has(key)}
                                  onToggle={() => toggleIn(setOpenVersions, key)}
                                  count={version.items.length}
                                  header={
                                    <Typography sx={{ fontSize: 13, fontWeight: 600 }}>
                                      {version.label}
                                    </Typography>
                                  }
                                  renderBody={() => (
                                    <>
                                      <ManualFileRowList
                                        items={version.items}
                                        shown={shown}
                                        target={targetId}
                                        selected={manualFile}
                                        onSelect={setManualFile}
                                        onMore={() => showMore(key)}
                                      />
                                    </>
                                  )}
                                />
                              );
                            })
                          }
                        />
                      </Paper>
                    ))
                  )}

              {rows !== null && rows.length === 0 && !(mode === "auto" && targetsError) && (
                <Box
                  sx={{
                    flex: 1,
                    display: "grid",
                    placeItems: "center",
                    p: 3,
                    textAlign: "center",
                  }}
                >
                  <Box>
                    <Typography sx={{ fontSize: 12.5, color: "text.secondary", mb: 1 }}>
                      {mode === "auto"
                        ? `本地没有能装这个${noun}的版本`
                        : manualLoader === "all"
                          ? "这个项目没有可下载的文件"
                          : `没有 ${loaderLabel(manualLoader)} 的文件`}
                    </Typography>
                    {mode === "auto" && (
                      <Button
                        size="small"
                        variant="outlined"
                        onClick={() => switchMode("manual")}
                      >
                        手动安装（自己选版本）
                      </Button>
                    )}
                  </Box>
                </Box>
              )}
            </Box>
          </PageLoaderGate>
        </Box>
      </Box>

      {/*
        右：升入式侧栏。
        样式逐条对齐 VersionDetail（原版安装那个面板）：头部 14/600 + 小 Chip +
        小「关闭」、Divider 分段、正文 p:2、底区 Divider + px2 pt1.5 pb2 + 满宽按钮、
        进度直接用同一个 `TaskProgress` —— 不要自己造一套。
      */}
      <SlidePanel open={panelOpen} width={PANEL_W}>
        <Stack
          direction="row"
          spacing={1}
          sx={{ alignItems: "center", px: 2, py: 1, flexShrink: 0 }}
        >
          <Typography sx={{ fontSize: 14, fontWeight: 600 }}>
            {mode === "auto" ? "选择文件版本" : "装到哪"}
          </Typography>
          {mode === "auto" && activeTarget && (
            <Chip
              size="small"
              label={`${activeTarget.gameVersion} · ${loaderLabel(activeTarget.loader)}`}
            />
          )}
          <Box sx={{ flex: 1 }} />
          <Button
            size="small"
            onClick={() => {
              if (mode === "auto") setActiveVersion(null);
              else setManualFile(null);
            }}
          >
            关闭
          </Button>
        </Stack>
        <Divider />

        {selectedFile && (
          <>
            <Box sx={{ px: 2, py: 1.5, flexShrink: 0 }}>
              <Typography sx={{ fontSize: 12, color: "text.secondary", mb: 0.5 }}>
                将下载
              </Typography>
              <Typography sx={{ fontSize: 12.5, wordBreak: "break-all" }}>
                {selectedFile.filename}
              </Typography>
              <Typography
                sx={{ fontSize: 11.5, color: "text.secondary", mt: 0.25, ...NUM }}
              >
                {formatBytes(selectedFile.size)} · {formatDate(selectedFile.published)}
              </Typography>
            </Box>
            <Divider />
          </>
        )}

        {/*
          依赖区块：放在正文最前面且不随列表滚动。
          放进下面的滚动列表里时，文件版本一多，依赖就被滚出视野。
        */}
        {showDepBlock && (
          <>
            <Box
              sx={{
                flexShrink: 0,
                px: 1,
                pt: 1,
                maxHeight: 180,
                overflowY: "auto",
                scrollbarGutter: "stable",
              }}
            >
              <DependencyBlock
                deps={depsShown ?? []}
                loading={depPending}
                error={depsErrorShown}
                onOpen={openDependency}
                onRetry={() => {
                  invalidate("resource_dependencies");
                  depsKeyRef.current = null;
                  setDepsReload((n) => n + 1);
                }}
              />
            </Box>
          </>
        )}

        {/*
          正文列表。两条规矩：
          1. 外框 16px 圆角、里面的行一律直角（和版本树 / 模组版本列表同一套）。
             这两处列表没有折叠标头，所以外框得自己加 —— 不加就是一排方角行贴在
             面板里，看着"没有圆角"。
          2. 转圈换成内容的那一刻要淡入（只淡入、不位移）。
        */}
        <Box
          sx={{
            flex: 1,
            minHeight: 0,
            overflow: "auto",
            scrollbarGutter: "stable",
            px: 1,
            py: 1,
          }}
        >
          {mode === "auto" ? (
            filesLoading ? (
              <Box sx={{ display: "grid", placeItems: "center", py: 6 }}>
                <Loading />
              </Box>
            ) : files.length === 0 ? (
              <Typography sx={{ fontSize: 12, color: "text.secondary", p: 1 }}>
                这个本地版本没有可用文件
              </Typography>
            ) : (
              <Box ref={panelBodyRef} sx={panelListSx}>
                {files.map((file) => (
                  <FileRow
                    key={file.fileId}
                    file={file}
                    target={targetId}
                    active={activeFile === file.fileId}
                    onClick={() => setActiveFile(file.fileId)}
                  />
                ))}
              </Box>
            )
          ) : installTargets.length === 0 ? (
            <Typography sx={{ fontSize: 12, color: "text.secondary", p: 1 }}>
              本地没有能装{noun}的版本
            </Typography>
          ) : (
            <Box ref={panelBodyRef} sx={panelListSx}>
              {installTargets.map((target) => (
                <InstallTargetRow
                  key={target.versionId}
                  target={target}
                  active={manualTarget === target.versionId}
                  onClick={() => setManualTarget(target.versionId)}
                />
              ))}
            </Box>
          )}
        </Box>

        <Divider />
        <Box sx={{ flexShrink: 0, px: 2, pt: 1.5, pb: 2 }}>
          <Stack spacing={1}>
            {installError && <ErrorState variant="banner" error={installError} />}

            {running && task && <TaskProgress task={task} />}

            <Stack direction="row" spacing={1}>
              {mode === "manual" && (
                <Button
                  variant="outlined"
                  onClick={() => void pickFolderAndDownload()}
                  disabled={!manualFile || starting || running}
                  sx={{ flexShrink: 0 }}
                >
                  自定义文件夹
                </Button>
              )}
              <Button
                variant="contained"
                onClick={install}
                disabled={!canDownload}
                sx={{ flex: 1, minWidth: 0 }}
              >
                {installLabel}
              </Button>
            </Stack>

            {showInstallHint && (
              <Typography sx={{ fontSize: 11.5, color: "text.secondary", px: 0.25 }}>
                {installHint}
              </Typography>
            )}
          </Stack>
        </Box>
      </SlidePanel>
    </Box>
  );

  /**
   * 依赖页是同一个组件的递归：整页退场 → 110ms 换页 → 依赖 Mod 的内容
   * 逐块落下。`initial={false}` 防止和外层（ModPage / 上一页）的入场叠加。
   * `lastChild` 让旧的一页在退场那 110ms 里继续有内容可画。
   */
  return (
    <PageTransition
      pageKey={child ? `dep:${child.item.source}:${child.item.id}` : "self"}
      initial={false}
      render={(key) =>
        key !== "self" && lastChild.current ? (
          <ResourceDetailPage
            item={lastChild.current.item}
            initialTarget={lastChild.current.target}
            initialMode={lastChild.current.mode}
            visitedProjects={lastChild.current.visited}
            // 依赖永远是 Mod（资源包的依赖也是如此，比如 Fresh Animations 要 EMF），
            // 所以这里固定 `mod`，不能把本页的 kind 传下去 —— 否则会把 Mod 当资源包，
            // 装到 `resourcepacks/` 里去，它永远不会被加载。
            kind="mod"
            onDone={() => setChild(null)}
          />
        ) : (
          detail
        )
      }
    />
  );
}

