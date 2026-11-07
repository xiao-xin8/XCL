import { useEffect, useState } from "react";
import { Box, Button, Chip, Paper, Stack, Typography, alpha } from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import BlockOutlined from "@mui/icons-material/BlockOutlined";
import DownloadOutlined from "@mui/icons-material/DownloadOutlined";
import LayersOutlined from "@mui/icons-material/LayersOutlined";
import LocalOfferOutlined from "@mui/icons-material/LocalOfferOutlined";
import ViewInArOutlined from "@mui/icons-material/ViewInArOutlined";
import { useUpdateActions, useUpdateState } from "../../lib/updateContext";

type AppInfo = {
  name: string;
  version: string;
  targetOs: string;
  targetArch: string;
  debug: boolean;
};

/**
 * 关于：官网式介绍，不是设置列表。
 *
 * 只讲「它是干什么的、用起来是什么感觉」，不写实现细节、不列别的项目。
 * 每张 `Paper` 自动算一个动画块（`motion/pageMotion` 的规则），
 * 进这一页时一块一块落下来，和别处同一套节奏 —— 不另写动画。
 */
const HIGHLIGHTS: {
  title: string;
  desc: string;
  icon: typeof LayersOutlined;
}[] = [
  {
    title: "全新架构",
    desc: "Tauri 2 与 Rust 打底，React 19、MUI 9、Vite 8 做界面，都是当下的主版本。",
    icon: LayersOutlined,
  },
  {
    title: "无广告",
    desc: "没有广告，也没有赞助解锁；界面里不会突然冒出不想看的东西。",
    icon: BlockOutlined,
  },
  {
    title: "完全免费",
    desc: "所有功能免费使用，不需要付费解锁，也没有隐藏收费项。",
    icon: LocalOfferOutlined,
  },
  {
    title: "多线程下载器",
    desc: "多个连接一起下，慢的源自动换掉，断了接着下，大文件不用干等。",
    icon: DownloadOutlined,
  },
];

/** 关于页只讲「能做什么」，不展开实现。两列排，一组一句话。 */
const FEATURES: { title: string; desc: string }[] = [
  {
    title: "版本与加载器",
    desc: "原版与 Fabric / Forge / NeoForge；装前预览、校验修复、改名换图标、收藏、单版本设置与版本隔离。",
  },
  {
    title: "下载与 Java",
    desc: "官方、镜像、自定义源都能选；多连接分片、断点续传、失败换源。缺 Java 会自动补齐并接着启动。",
  },
  {
    title: "启动与账号",
    desc: "内存、窗口、参数、实时日志；离线与第三方登录、3D 皮肤预览、账号一键切换。",
  },
  {
    title: "资源与整合包",
    desc: "Mod、资源包、光影、整合包都能搜；平台与版本筛选、依赖提示、收藏。整合包装完直接生成新版本。",
  },
  {
    title: "联机",
    desc: "房间名加密码开房，一条邀请码加入，进服地址一键复制。不需要管理员权限，也不建虚拟网卡。",
  },
  {
    title: "界面与维护",
    desc: "主题、强调色、背景与透明度都能调；设置可导出导入，缓存可清理，程序自己更新自己。",
  },
];

const EYEBROW_SX = {
  fontSize: 10.5,
  fontWeight: 700,
  letterSpacing: "0.1em",
  color: "brand.ink",
  mb: 0.5,
} as const;

const H2_SX = {
  fontSize: 15.5,
  fontWeight: 800,
  letterSpacing: -0.2,
} as const;

export default function AboutSection() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const { view } = useUpdateState();
  const { check } = useUpdateActions();
  const checking = view?.phase === "checking";

  useEffect(() => {
    let alive = true;
    void invoke<AppInfo>("app_info")
      .then((data) => {
        if (alive) setInfo(data);
      })
      .catch(() => {
        // 拿不到版本号不影响这一页
      });
    return () => {
      alive = false;
    };
  }, []);

  return (
    <Box sx={{ display: "flex", flexDirection: "column", gap: 1.5, pb: 1 }}>
      <Paper
        variant="outlined"
        sx={{ position: "relative", overflow: "hidden", borderRadius: 1, p: 3 }}
      >
        {/* 一层强调色渐变：换配色时这张卡自己会跟着变 */}
        <Box
          aria-hidden
          sx={{
            position: "absolute",
            inset: 0,
            background: (t) =>
              `linear-gradient(135deg, ${alpha(t.palette.primary.main, 0.55)} 0%, ${alpha(
                t.palette.primary.main,
                0.12,
              )} 55%, transparent 100%)`,
          }}
        />
        <Box
          aria-hidden
          sx={{
            position: "absolute",
            right: 26,
            bottom: -34,
            fontSize: 138,
            lineHeight: 1,
            fontWeight: 900,
            color: "primary.main",
            opacity: 0.075,
            userSelect: "none",
            pointerEvents: "none",
          }}
        >
          鑫
        </Box>

        <Box
          sx={{
            position: "relative",
            display: "flex",
            alignItems: "center",
            gap: 2,
            flexWrap: "wrap",
          }}
        >
          <Box
            component="img"
            src="/xcl-icon.png"
            alt=""
            aria-hidden="true"
            draggable={false}
            sx={{
              width: 64,
              height: 64,
              flexShrink: 0,
              objectFit: "contain",
              userSelect: "none",
            }}
          />
          <Box sx={{ minWidth: 0 }}>
            <Box sx={{ display: "flex", alignItems: "center", gap: 1, flexWrap: "wrap" }}>
              <Typography sx={{ fontSize: 26, fontWeight: 800, letterSpacing: -0.5 }}>
                {info?.name ?? "Xin Craft Launcher"}
              </Typography>
              {info && (
                <Chip
                  size="small"
                  color="primary"
                  label={`v${info.version}`}
                  sx={{ height: 22, fontSize: 11, fontWeight: 700 }}
                />
              )}
            </Box>
            <Typography sx={{ fontSize: 13, color: "text.secondary", mt: 0.5 }}>
              一个安静、顺手的 Minecraft 启动器。
            </Typography>
          </Box>
        </Box>

        {info && (
          <Stack
            direction="row"
            sx={{ position: "relative", mt: 2.5, flexWrap: "wrap", gap: 0.75 }}
          >
            <Chip
              size="small"
              label={`${info.targetOs} / ${info.targetArch}`}
              sx={{ height: 20, fontSize: 10.5 }}
            />
            <Button
              size="small"
              variant="outlined"
              onClick={() => void check(false)}
              disabled={checking}
              sx={{ height: 20, minHeight: 20, py: 0, px: 1, fontSize: 10.5 }}
            >
              检查更新
            </Button>
          </Stack>
        )}
      </Paper>

      <Paper variant="outlined" sx={{ borderRadius: 1, p: 3 }}>
        <Typography sx={EYEBROW_SX}>关于 XCL</Typography>
        <Typography sx={H2_SX}>先把启动这件事做好</Typography>
        <Typography sx={{ fontSize: 12.5, color: "text.secondary", lineHeight: 1.95, mt: 1 }}>
          XCL 是一个 Minecraft 启动器。找版本、装模组、点启动，然后进游戏 ——
          每天要做的就这几件事，把它做顺就够了。
        </Typography>
        <Typography sx={{ fontSize: 12.5, color: "text.secondary", lineHeight: 1.95, mt: 1 }}>
          所有东西都在本地。没有正版账号也能离线玩；想登录就登录 ——
          你的存档和账号只属于你自己。
        </Typography>
      </Paper>

      <Box
        sx={{
          display: "grid",
          gridTemplateColumns: "repeat(auto-fill, minmax(220px, 1fr))",
          gap: 1.5,
        }}
      >
          {HIGHLIGHTS.map((item) => {
            const Icon = item.icon;
            return (
              <Paper
                key={item.title}
                variant="outlined"
                sx={{ borderRadius: 1, p: 2.25, minWidth: 0 }}
              >
                <Box
                  sx={{
                    width: 28,
                    height: 28,
                    borderRadius: 1.25,
                    display: "grid",
                    placeItems: "center",
                    bgcolor: (t) => alpha(t.palette.primary.main, 0.18),
                    color: "brand.ink",
                    mb: 1.25,
                  }}
                >
                  <Icon sx={{ fontSize: 16 }} />
                </Box>
                <Typography sx={{ fontSize: 13.5, fontWeight: 700, mb: 0.5 }}>
                  {item.title}
                </Typography>
                <Typography sx={{ fontSize: 11.5, color: "text.secondary", lineHeight: 1.8 }}>
                  {item.desc}
                </Typography>
              </Paper>
            );
          })}
      </Box>

      <Paper variant="outlined" sx={{ borderRadius: 1, p: 3 }}>
        <Typography sx={EYEBROW_SX}>功能</Typography>
        <Typography sx={H2_SX}>能做这些事</Typography>
        <Box
          sx={{
            display: "grid",
            gridTemplateColumns: "repeat(auto-fill, minmax(240px, 1fr))",
            columnGap: 3,
            rowGap: 1.5,
            mt: 2,
          }}
        >
          {FEATURES.map((item) => (
            <Box key={item.title} sx={{ minWidth: 0 }}>
              <Typography sx={{ fontSize: 12.5, fontWeight: 700, mb: 0.4 }}>
                {item.title}
              </Typography>
              <Typography sx={{ fontSize: 11.5, color: "text.secondary", lineHeight: 1.8 }}>
                {item.desc}
              </Typography>
            </Box>
          ))}
        </Box>
      </Paper>

      <Paper
        variant="outlined"
        sx={{
          borderRadius: 1,
          p: 3,
          display: "flex",
          alignItems: "flex-start",
          gap: 1.75,
          background: (t) =>
            `linear-gradient(135deg, ${alpha(t.palette.primary.main, 0.1)}, transparent 58%), ${
              t.palette.background.paper
            }`,
        }}
      >
        <Box
          sx={{
            width: 42,
            height: 42,
            flexShrink: 0,
            borderRadius: 1.25,
            display: "grid",
            placeItems: "center",
            bgcolor: (t) => alpha(t.palette.primary.main, 0.18),
            color: "brand.ink",
          }}
        >
          <ViewInArOutlined sx={{ fontSize: 25 }} />
        </Box>
        <Box sx={{ minWidth: 0 }}>
          <Typography sx={EYEBROW_SX}>鸣谢</Typography>
          <Typography sx={H2_SX}>感谢 Minecraft</Typography>
          <Typography sx={{ fontSize: 12.5, color: "text.secondary", lineHeight: 1.9, mt: 0.75 }}>
            感谢 Mojang Studios 创造了 Minecraft，也感谢每一位让这个方块世界继续生长的
            玩家与创作者。
          </Typography>
          <Typography sx={{ fontSize: 10.5, color: "text.secondary", lineHeight: 1.75, mt: 1 }}>
            Minecraft 是 Mojang Studios 的商标，XCL 与 Mojang、Microsoft 没有隶属关系。
          </Typography>
        </Box>
      </Paper>

      <Box
        sx={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: 1,
          flexWrap: "wrap",
          px: 0.5,
          pt: 0.5,
        }}
      >
        <Typography sx={{ fontSize: 10.5, color: "text.secondary" }}>
          © 2026 鑫乐云科技 · MIT
        </Typography>
        <Typography sx={{ fontSize: 10.5, color: "text.secondary" }}>
          {info?.name ?? "Xin Craft Launcher"}
        </Typography>
      </Box>
    </Box>
  );
}
