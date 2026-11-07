/**
 * 跨组件共享的数据形状。
 *
 * 这些类型是 Rust 侧视图模型的镜像（字段名与 `#[serde(rename_all = "camelCase")]`
 * 对齐），所以只在一处定义 —— 之前散在各个组件里，改后端一个字段要翻三个文件。
 */

// ---------- 下载任务（对应 xcl-service::download::TaskView） ----------

export type TaskPhase = "running" | "done" | "failed" | "cancelled";

export type TaskView = {
  taskId: string;
  versionId: string;
  /**
   * 抽屉里显示的名字。版本安装是 `null`（用 `versionId`）；
   * 单文件下载（Mod）给文件名 —— 否则会显示成 `mod:xxx` 这种内部 id。
   */
  title: string | null;
  /**
   * 加载器安装时指回父版本（原版）的 id。
   *
   * 加载器任务的 `versionId` 是 `fabric-loader-x-y` 这种新实例 id，
   * 详情面板靠这个字段认出「这条任务是我发起的」。
   */
  parentId: string | null;
  /**
   * 单文件下载落到哪（目标版本 id 或自定义目录）。版本安装为 `null`。
   *
   * 同一个 Mod 文件装到两个版本是两条任务，`versionId` 完全相同，
   * 只有靠「`versionId` + `target`」才能把两条进度分开。
   */
  target: string | null;
  phase: TaskPhase;
  totalFiles: number;
  finishedFiles: number;
  totalBytes: number;
  doneBytes: number;
  /** 0 ~ 1，已按字节算好 */
  fraction: number;
  speedBps: number;
  etaSeconds: number | null;
  downloaded: number;
  skipped: number;
  failed: number;
  failures: string[];
  message: string | null;
  startedAtMs: number;
};

// ---------- 版本清单（对应 xcl-service::versions） ----------

/** 版本图标分类。后端判定，前端只拿来查图标，不做任何推断 */
export type VersionKind =
  | "vanilla"
  | "snapshot"
  | "oldAlpha"
  | "oldBeta"
  | "forge"
  | "neoForge"
  | "fabric"
  | "optiFine"
  | "liteLoader"
  | "unknown";

export type VersionItem = {
  id: string;
  releaseTime: string | null;
  url: string;
  latest: boolean;
  /** 下载清单里只有原版，所以只会是 vanilla / snapshot / oldAlpha / oldBeta */
  kind: VersionKind;
  /** 本地是否已装好。后端每次实时算，不受清单缓存影响 */
  installed: boolean;
  /**
   * 这个原版上已经装过哪些加载器，后端直接给展示名（`["Fabric"]`、`["Forge","NeoForge"]`）。
   *
   * 独立于 `installed`：装加载器只产生一条自包含版本，原版目录可能压根没有。
   * 名字由后端算（`xcl_loader::loader_label_of`），前端不再认 id 前缀。
   */
  installedLoaders: string[];
};

export type VersionGroup = {
  key: string;
  label: string;
  latest: boolean;
  items: VersionItem[];
};

export type VersionListView = {
  releases: VersionGroup[];
  snapshots: VersionGroup[];
  old: VersionGroup[];
  fools: VersionGroup[];
  releaseCount: number;
  snapshotCount: number;
  oldCount: number;
  foolCount: number;
  source: string;
  fetchedAtMs: number;
  fromCache: boolean;
};

// ---------- 安装清单（对应 xcl-service::install） ----------

export type PlanGroup = {
  kind: string;
  label: string;
  count: number;
  size: number;
};

export type InstallPreview = {
  versionId: string;
  versionType: string | null;
  releaseTime: string | null;
  javaMajor: number | null;
  mainClass: string | null;
  groups: PlanGroup[];
  totalCount: number;
  totalSize: number;
  /**
   * 这份清单的目标版本已经装好了。
   *
   * 加载器场景里 `versionId` 是 `fabric-loader-x-y` 这个实例 id，
   * 界面据此把主按钮从「安装」换成「校验并修复」。
   */
  installed: boolean;
};

// ---------- 模组加载器（对应 xcl-service::loader） ----------

/**
 * 可装的加载器类型。
 *
 * 下拉一律照后端返回渲染 —— Fabric / Forge / NeoForge 走同一条界面路径，
 * 不给任何类型开特殊分支（能力差异由后端在安装那一步负责说清楚）。
 */
export type LoaderKind = "fabric" | "forge" | "neoforge";

/** `loader_kinds` 返回的一条。 */
export type LoaderKindView = {
  kind: LoaderKind;
  /** 展示名，如 `NeoForge` */
  label: string;
};

/** 安装时指定的加载器（IPC 入参 `loader`）。 */
export type LoaderRef = {
  kind: LoaderKind;
  version: string;
};

export type LoaderItem = {
  version: string;
  stable: boolean;
  /** 这个加载器版本 + 当前原版的实例已经装好了（后端扫版本目录算的） */
  installed: boolean;
};

/** 同一 minor 的版本组，例如 `0.16.x`。分组由后端算好。 */
export type LoaderGroup = {
  key: string;
  /** 这一组是否包含全局最新版（组头挂「最新」标签） */
  latest: boolean;
  items: LoaderItem[];
};

export type LoaderListView = {
  kind: string;
  /** 默认选中项：最新稳定版 */
  latest: string | null;
  groups: LoaderGroup[];
};

// ---------- 模组平台（对应 xcl-service::resources） ----------

/** 两个源是互斥的：一次只查一个 */
export type ModSource = "modrinth" | "curseforge";

/**
 * 资源类型。搜索 / 详情 / 文件列表三个环节三者完全同构，
 * 只有平台侧的类型值不同 —— 所以列表页是同一个组件，`kind` 决定查什么。
 *
 * 落点各不相同：`mods/`、`resourcepacks/`、`shaderpacks/`，路径只在后端拼。
 */
/**
 * 资源类型。
 *
 * `modpack` 比较特殊：它不往任何本地版本里落文件，而是新建一个实例，
 * 走 `modpack_install` 那条独立链路（列表页仍复用同一套）。
 */
export type ResourceKind = "mod" | "resourcepack" | "shader" | "modpack";

/** 排序：两个平台的枚举值由后端翻译，前端只认这三个 */
export type ModSort = "relevance" | "downloads" | "updated";

export type ModItem = {
  source: ModSource;
  /** 平台内的项目 id */
  id: string;
  /** 项目主页（浏览器打开 / 手动下载兜底） */
  url: string;
  title: string;
  description: string;
  author: string;
  iconUrl: string | null;
  downloads: number;
  /** ISO 8601 */
  updated: string | null;
  /** 支持的加载器：fabric / forge / neoforge / quilt */
  loaders: string[];
};

/**
 * 一条收藏。
 *
 * `info` 是收藏那一刻的卡片快照：收藏页要能离线打开、要能搜自己的名字，
 * 所以不能只存 id 再回平台查。快照只用于渲染，点进去仍是实时数据。
 */
export type FavoriteEntry = {
  source: ModSource;
  kind: ResourceKind;
  id: string;
  /** 收藏时间（毫秒），列表按它倒序 */
  savedMs: number;
  info: ModItem;
};

export type ModSearchView = {
  items: ModItem[];
  total: number;
  offset: number;
  hasMore: boolean;
};

// ---------- 资源详情 / 安装（对应 xcl-service::resources） ----------

/** 文件里声明的依赖（随文件列表一起回来，不需要选安装位置）。 */
export type DependencyRef = {
  projectId: string;
  versionId: string | null;
  relation: DependencyRelation;
};

/** 一个可下载的文件版本。 */
export type ResourceFile = {
  fileId: string;
  versionNumber: string;
  filename: string;
  size: number;
  sha1: string | null;
  url: string;
  /** ISO 8601 */
  published: string | null;
  /** release / beta / alpha */
  releaseType: string;
  /** 支持的 MC 版本（CF 那边已经剥掉混进来的加载器名） */
  gameVersions: string[];
  loaders: string[];
  /** 需要展示的依赖数量（内嵌库 / 工具不算）。首屏就能知道要不要出依赖区 */
  dependencyCount: number;
  /** 原始依赖引用（手动模式不用选安装位置就能提示“依赖谁”） */
  dependencies: DependencyRef[];
};

export type ResourceFilesView = {
  items: ResourceFile[];
  /** 只有自动模式给；手动模式后端固定给 null（不替选） */
  defaultFileId: string | null;
  /** 自动模式：默认文件解析好的依赖，和文件列表一起回来；手动模式为空 */
  dependencies: ResourceDependency[];
  /** 是否因为数量上限被截断 */
  truncated: boolean;
};

/** 依赖关系：只有这三种会进界面，内嵌库 / 工具不展示 */
export type DependencyRelation = "required" | "optional" | "incompatible";

/**
 * 右栏依赖区块的一行（对应 `xcl-service::resources::ResourceDependencyView`）。
 *
 * `project` 是完整的模组列表视图 —— 点「安装依赖」时直接拿它构造依赖 Mod
 * 的详情页，不再多发一次项目请求。
 */
export type ResourceDependency = {
  project: ModItem;
  relation: DependencyRelation;
  /** 对当前本地版本解析出的文件；incompatible 或没有兼容文件时为 null */
  file: ResourceFile | null;
  /** 目标 mods/ 里已经有这个文件 */
  installed: boolean;
  /** 解析不出来时的说明（例如「没有支持 1.20.1 · fabric 的文件」） */
  reason: string | null;
};

/** 项目详情里的一张截图（对应 `ProjectImageView`）。 */
export type ProjectImage = {
  url: string;
  title: string | null;
};

/** 项目详情里的一个外链（对应 `ProjectLinkView`）。 */
export type ProjectLink = {
  label: string;
  url: string;
};

/**
 * 项目详情（对应 `ResourceProjectView`）。
 *
 * CurseForge 的公开接口拿不到正文，`body` 固定为 null；
 * 其余字段两个源能拿到的都填，拿不到的保持 null / 空数组。
 */
export type ResourceProject = {
  source: ModSource;
  id: string;
  url: string;
  title: string;
  description: string;
  /** Markdown 正文；CF 为 null */
  body: string | null;
  iconUrl: string | null;
  author: string;
  downloads: number;
  followers: number;
  published: string | null;
  updated: string | null;
  categories: string[];
  loaders: string[];
  clientSide: string | null;
  serverSide: string | null;
  license: string | null;
  gallery: ProjectImage[];
  links: ProjectLink[];
};

/**
 * 详情页左侧的一行。
 *
 * 这是一行一个「本地版本」，不是远程文件 —— 后端已经把「这个 Mod 装不上我」
 * 的本地版本滤掉了，前端只管画。
 */
export type LocalTarget = {
  versionId: string;
  gameVersion: string;
  /** fabric / forge / neoforge */
  loader: string;
  lastPlayedMs: number;
  /** 目标 mods/ 里已经有这个 Mod 了 */
  installed: boolean;
};

/** 三级里的最内层：一个具体 MC 版本下的全部文件。 */
export type ResourceFileGroup = {
  /** 具体版本号，比如 `1.20.1` */
  label: string;
  items: ResourceFile[];
};

/** 三级里的第一层：一条版本线（`1.20`），下面是具体的 MC 版本。 */
export type ResourceFileLine = {
  label: string;
  /** 这条线下的文件条目数（同一个文件在不同版本里各算一次） */
  count: number;
  versions: ResourceFileGroup[];
};

export type ResourceGroupsView = {
  lines: ResourceFileLine[];
  /** 文件总数（一个文件可能属于多个 MC 版本，会重复出现） */
  total: number;
};

/** 手动模式右栏的一行：本地所有能装 Mod 的版本。 */
export type InstallTarget = {
  versionId: string;
  gameVersion: string;
  loader: string;
  lastPlayedMs: number;
};

// ---------- 本地已安装版本（对应 xcl-service::local::LocalVersion） ----------

export type LocalVersion = {
  id: string;
  versionType: string;
  /** 从版本 JSON 的库名 / 主类识别出的分类，用于选图标 */
  kind: VersionKind;
  /** 能不能装 Mod（Forge / Fabric / …）。纯净版为 false */
  modable: boolean;
  /** 1.5 及更早的版本只有材质包，没有资源包 */
  texturePacks: boolean;
  /** 1.7+ 才会生成 logs 目录，更老的版本没有 */
  hasLogs: boolean;
  inheritsFrom: string | null;
  hasClientJar: boolean;
  /** 这个版本对应的 MC 版本号（`1.20.1`），下载页的「版本筛选」用它当选项 */
  gameVersion: string | null;
  /**
   * 装了加载器的哪个版本（`net.fabricmc:fabric-loader:0.20.1` → `0.20.1`）。
   *
   * 从版本 JSON 的 `libraries` 反推，和目录名无关 —— 版本被改名也认得出。
   * 纯净版为 `null`。
   */
  loaderVersion: string | null;
  /** 沿 inheritsFrom 找到 jar 了，可以启动 */
  launchable: boolean;
  javaMajor: number | null;
  mainClass: string | null;
  modifiedMs: number;
  valid: boolean;
  error: string | null;
  /** 玩家自己写的说明（存在 `XCL/instance.toml`） */
  description?: string | null;
  /** 收藏的版本排最前 */
  favorite?: boolean;
  /** 自定义图标（PNG data URL） */
  icon?: string | null;
};

// ---------- 版本隔离（对应 xcl-service::isolation） ----------

export type IsolationSource = "user" | "auto" | "policy";

export type IsolationPolicy =
  | "off"
  | "modable"
  | "nonRelease"
  | "nonReleaseOrModable"
  | "all";

export type IsolationView = {
  versionId: string;
  enabled: boolean;
  source: IsolationSource;
  /** 来源的中文说明，直接显示 */
  sourceLabel: string;
  /** 隔离后的游戏目录绝对路径 */
  gameDir: string;
  /** 相对数据根的显示名 */
  gameDirLabel: string;
  policy: IsolationPolicy;
  /** 全局策略的中文说明 */
  policyLabel: string;
};

// ---------- 启动（对应 xcl-service::launch） ----------

export type LaunchPhase =
  | "preparing"
  | "running"
  | "exited"
  | "failed"
  | "cancelled";

export type LaunchStatus = {
  sessionId: string;
  versionId: string;
  phase: LaunchPhase;
  message: string;
  exitCode: number | null;
  durationMs: number;
  javaPath: string | null;
  javaMajor: number | null;
};

export type LaunchLog = {
  sessionId: string;
  stream: "stdout" | "stderr";
  text: string;
};

/** 后端推的是带 `kind` 标签的联合体 */
export type LaunchEvent =
  | ({ kind: "status" } & LaunchStatus)
  | ({ kind: "log" } & LaunchLog);

export type JavaInfo = {
  path: string;
  version: string;
  major: number;
};

// ---------- Java 运行时（对应 xcl-service::java_runtime::JavaComponent） ----------

/** 本机探测到的一个 Java（对应 xcl-service::java_runtime::SystemJava） */
export type SystemJava = {
  path: string;
  version: string;
  major: number;
  /** 是否在 data/java/ 下（由 XCL 下载管理） */
  managed: boolean;
};

/** 清理强度：由后端按当前进程的权限自动决定，前端只负责显示。 */
export type TrimMode = "deep" | "standard";

/** 「清理内存」的结果（设置 → 高级 手动触发一次）。 */
export type TrimReport = {
  /** `deep` = 系统级清理；`standard` = 逐进程清理（没有管理员权限时） */
  mode: TrimMode;
  /** 深度模式是「系统清理项数」，标准模式是「清到的进程数」 */
  count: number;
  /** 清理前后可用物理内存的差值（MB），通常为正 */
  freedMb: number;
};

/** 「清理未使用资源」的结果（对应 `commands::storage_unused_*`）。 */
export type UnusedReport = {
  librariesFiles: number;
  librariesBytes: number;
  indexesFiles: number;
  indexesBytes: number;
  objectsFiles: number;
  objectsBytes: number;
};

export type JavaComponent = {
  /** 组件名，如 `jre-legacy` */
  name: string;
  /** 展示名，如 `Java 8（jre-legacy）` */
  label: string;
  major: number;
  /** 官方版本号，如 `8u51-cacert462b08` */
  version: string;
  installed: boolean;
  path: string | null;
  sizeOnDisk: number | null;
  /** 本机已装同主版本的 Java（不需要再下 143MB 了） */
  systemJava: SystemJava | null;
};

/** Java 任务在共享任务表里的 id 前缀 */
export const JAVA_TASK_PREFIX = "java:";

export function isJavaTask(versionId: string): boolean {
  return versionId.startsWith(JAVA_TASK_PREFIX);
}

// ---------- 账号（对应 xcl-service::auth） ----------

/** 登录方式。微软登录不在支持范围内（需要正版账号才能验证链路） */
export type AccountKind = "offline" | "authlib";

export type AccountView = {
  id: string;
  kind: AccountKind;
  /** 离线 = 玩家名；第三方 = 角色名 */
  username: string;
  uuid: string;
  /** 第三方登录的认证服务器地址 */
  serverUrl: string | null;
  /** 登录邮箱 */
  email: string | null;
  /** 是否存了密码（决定「刷新登录」按钮亮不亮） */
  hasPassword: boolean;
  /** 是否是当前账号 */
  active: boolean;
  /** 离线账号手动选的皮肤模型；null / 缺省 = 按 UUID 自动 */
  skinModel?: SkinModel | null;
};

/** 第三方服务器上的一个角色 */
export type AuthlibProfile = {
  /** 32 位十六进制、无横杠 */
  id: string;
  name: string;
};

// ---------- 皮肤（对应 xcl-service::skin） ----------

/** 皮肤模型：default = 粗手臂（Steve），slim = 细手臂（Alex） */
export type SkinModel = "default" | "slim";

/** 皮肤数据来源：服务器拉来的，还是内置默认皮肤 */
export type SkinSource = "server" | "offlineDefault";

/** `skin_current` 的返回：位图一律 base64（PNG，不带 data: 前缀） */
export type SkinRender = {
  skin: string;
  cape: string | null;
  model: SkinModel;
  source: SkinSource;
};

/** `account_add_authlib` 的两种结果 */
export type AuthlibLoginOutcome =
  | ({ outcome: "loggedIn" } & AccountView)
  | {
      outcome: "needProfile";
      profiles: AuthlibProfile[];
      serverUrl: string;
    };

// ---------- 错误（对应 xcl-core::AppError） ----------

export type AppError = {
  code: string;
  title: string;
  message: string;
  hint?: string;
  retryable: boolean;
};

/** IPC 抛出来的不一定是我们约定的结构（可能是字符串、可能是 Tauri 自己的错误）。 */
export function isAppError(value: unknown): value is AppError {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as AppError).title === "string"
  );
}

/** 把任意抛出物收敛成可渲染的 `AppError`，前端各处不用各写一遍。 */
export function toAppError(value: unknown, fallbackTitle: string): AppError {
  if (isAppError(value)) return value;
  return {
    code: "unknown",
    title: fallbackTitle,
    message: String(value),
    retryable: true,
  };
}

// ---------- 配置（对应 xcl-core::config::Config） ----------
// 隔离策略复用上面版本隔离那套 `IsolationPolicy`，不重复定义。

export type SourceStrategy = "mirrorFirst" | "auto" | "officialFirst";

/** 强调色（定义在 `theme.ts`，这里转出去给配置用） */
export type { AccentKey } from "../theme";

/** 启动时打开哪一页 */
export type StartPage = "last" | "launch" | "download" | "setting";

/** 主题模式 */
export type ThemePreference = "system" | "light" | "dark";

/** 动画强度 */
export type AnimationPreference = "system" | "on" | "off";

/** 游戏进程优先级（只有 Windows 生效） */
export type ProcessPriority = "low" | "normal" | "high";

/** 日志级别 */
export type LogLevel = "error" | "warn" | "info" | "debug" | "trace";

/** 带 tag 的枚举：unit 变体只有 `kind`，自定义镜像多一个 `value` */
export type MirrorProvider =
  | { kind: "official" }
  | { kind: "bmclapi" }
  | { kind: "custom"; value: string };

/** 内存分配方式：自动 / 手动 */
export type MemoryMode = "auto" | "custom";

export type AppConfig = {
  schemaVersion: number;
  mirror: {
    provider: MirrorProvider;
    downloadStrategy: SourceStrategy;
    versionStrategy: SourceStrategy;
  };
  download: {
    concurrency: number;
    speedLimitKbps: number;
    splitEnabled: boolean;
    splitConnections: number;
    splitTotalConnections: number;
    /** 单个地址失败后重试几次（不含第一次尝试） */
    retries: number;
  };
  java: {
    /** 空 = 自动探测 */
    manualPath: string | null;
    autoDownload: boolean;
  };
  launch: {
    /** 已废弃：身份一律来自账号表，界面不展示 */
    playerName: string;
    memoryMode: MemoryMode;
    memoryMb: number;
    windowWidth: number;
    windowHeight: number;
    fullscreen: boolean;
    extraJvmArgs: string;
    extraGameArgs: string;
    language: string;
    preLaunchCommand: string;
    waitCommand: boolean;
    isolation: IsolationPolicy;
    /** 游戏进程优先级（只有 Windows 生效） */
    priority: ProcessPriority;
    /** 把 java.exe 和启动器标成「使用高性能显卡」（只有 Windows 生效） */
    highPerfGpu: boolean;
    /** 启动游戏前清理一次内存（换出各进程不活跃的工作集） */
    trimMemory: boolean;
  };
  /** 日志级别，改完立刻生效 */
  log: { level: LogLevel };  ui: {
    rememberWindow: boolean;
    /** 关闭主窗口时：background = 留在右下角继续跑，quit = 直接退出 */
    closeAction: "background" | "quit";
    /** 上次停留的页面（真实 tab key） */
    lastTab: string;
    /** 启动时打开哪一页 */
    startPage: StartPage;
    themeMode: ThemePreference;
    accent: import("../theme").AccentKey;
    /** 自定义强调色（`#rrggbb`）；null = 用 accent 预设 */
    accentCustom: string | null;
    /** 界面不透明度 0–100（卡片、侧栏、弹出层统一） */
    surfaceOpacity: number;
    /** 毛玻璃模糊半径（px）0–20 */
    surfaceBlur: number;
    /** 背景图文件名（在 data/background/ 下）；null = 不用背景图 */
    backgroundImage: string | null;
    /** 背景图蒙版不透明度 0–100（0 = 原图） */
    backgroundOverlay: number;
    /** 背景图自身模糊半径 0–20 px */
    backgroundBlur: number;
    animation: AnimationPreference;
    windowWidth: number;
    windowHeight: number;
    showConnectTab: boolean;
    /** 首页最后选中的版本 id；空串 = 还没选过，回退到列表第一个 */
    selectedVersion: string;
  };
  /** 自更新 */
  update: UpdateConfig;
  /** 联机：房间身份（房间名 + 密码），与游戏端口无关 */
  connect: {
    room: string;
    secret: string;
    /** 是否已经看过「离线账号进不去」的首次提示（只看一次） */
    hintSeen: boolean;
  };
};

/** 发布频道 */
export type UpdateChannel = "stable" | "preview";

/** 自更新配置（`[update]`） */
export type UpdateConfig = {
  channel: UpdateChannel;
  /** 启动后自动检查 */
  autoCheck: boolean;
  autoDownload: boolean;
  /** 忽略的版本 */
  skippedVersion: string;
  lastCheckAt: number;
  lastSeenVersion: string;
  /** 强制更新的持久化状态：非空表示必须升到这个版本 */
  forceVersion: string;
  forceTitle: string;
  forceNotes: string;
  forceChangelog: string[];
};

// ---------- 自更新（对应 xcl_service::update） ----------

export type UpdatePhase =
  | "idle"
  | "checking"
  | "upToDate"
  | "available"
  | "downloading"
  | "verifying"
  | "ready"
  | "applying"
  | "failed";

/**
 * 一次更新的完整视图。所有派生量（百分比、速度、剩余时间）后端都算好了，
 * 前端只负责格式化。
 */
export type UpdateView = {
  phase: UpdatePhase;
  currentVersion: string;
  latestVersion: string | null;
  notes: string | null;
  /** 逐条更新内容 */
  changelog: string[];
  /** 版本标题，没有就用版本号 */
  title: string | null;
  notesUrl: string | null;
  /** 清单标了 critical：文案上建议尽快更新 */
  critical: boolean;
  belowMinSupported: boolean;
  /** 强制更新：整个界面只允许操作更新 */
  forced: boolean;
  forceTitle: string | null;
  forceNotes: string | null;
  forceChangelog: string[];
  skippedVersion: string | null;
  totalBytes: number;
  doneBytes: number;
  /** 0 ~ 1 */
  fraction: number;
  speedBps: number;
  etaSeconds: number | null;
  error: AppError | null;
  /** 安装目录能不能写 */
  canAutoUpdate: boolean;
  installDir: string;
};

/** 目录布局（对应 commands::PathsInfo） */
export type PathsInfo = {
  root: string;
  versions: string;
  libraries: string;
  assets: string;
  config: string;
  cache: string;
};

/**
 * 设置页「内存分配」的实时数据（对应 `commands::memory_stats`）。
 *
 * 自动模式按「这个版本的 mod 数 + 当前可用内存」算（阶梯分配），
 * 所以它会随系统负载和版本内容浮动 —— 这是刻意的，不是 bug。
 */
export type MemoryStats = {
  totalMb: number;
  /** 已被占用（总量 − 可用） */
  usedMb: number;
  availableMb: number;
  /** 当前设置下游戏会拿到的内存 */
  gameMb: number;
  /** 自动模式下是按多少个 mod 算的 */
  modCount: number;
  /** `true` = 自动分配 */
  auto: boolean;
  /** 自动模式参照的版本；`null` = 没选版本，按纯原版算 */
  versionId: string | null;
};

// ---------- 单个版本的启动覆盖（对应 xcl_service::version_settings） ----------

/** `null` = 跟随全局；非 null = 只对这个版本生效 */
export type LaunchOverride = {
  memoryMode: MemoryMode | null;
  memoryMb: number | null;
  javaPath: string | null;
  jvmArgs: string | null;
  gameArgs: string | null;
  windowWidth: number | null;
  windowHeight: number | null;
  fullscreen: boolean | null;
  language: string | null;
  preLaunchCommand: string | null;
  waitCommand: boolean | null;
};

/** 合并后的最终值（后端算好，前端只显示） */
export type LaunchValues = {
  memoryMode: MemoryMode;
  memoryMb: number;
  javaPath: string | null;
  jvmArgs: string;
  gameArgs: string;
  windowWidth: number;
  windowHeight: number;
  fullscreen: boolean;
  language: string;
  preLaunchCommand: string;
  waitCommand: boolean;
};

export type VersionSettingsView = {
  versionId: string;
  overrides: LaunchOverride;
  effective: LaunchValues;
  global: LaunchValues;
};

/** 版本占用与内容统计 */
export type VersionStats = {
  sizeBytes: number;
  mods: number;
  saves: number;
  screenshots: number;
  resourcepacks: number;
  lastPlayedMs: number | null;
  gameDir: string;
};
