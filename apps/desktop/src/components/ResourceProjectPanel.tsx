import { useState } from "react";
import {
  Box,
  Button,
  ButtonBase,
  Chip,
  Paper,
  Stack,
  Typography,
  alpha,
} from "@mui/material";
import ExpandMoreIcon from "@mui/icons-material/ExpandMore";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeRaw from "rehype-raw";
import rehypeSanitize from "rehype-sanitize";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { AppError, ModItem, ResourceProject } from "../lib/types";
import { formatCount, formatDate } from "../lib/format";
import { platformLoaderLabels } from "../lib/loaders";
import { CARD_MOTION, EASE } from "../motion/tokens";
import ErrorState from "./ErrorState";
import Loading from "./Loading";
import Reveal from "./settings/Reveal";

/** 打开外部链接：失败只当没点，不把 Promise 的 rejection 丢到控制台。 */
function openExternal(url: string): void {
  void openUrl(url).catch(() => undefined);
}

/**
 * 详情页左列的「项目详情」卡。
 *
 * 默认折叠，只露摘要 + 下载量/更新时间；点整行展开 / 收回。
 * 展开区限高 38vh 且内部滚动 —— 正文再长也不能把下面的版本列表顶没。
 *
 * 展开 / 收起用 `Reveal`（grid-template-rows: 0fr ↔ 1fr），参数全复用
 * `CARD_MOTION.collapse` 与 `EASE.outFluent`，不新增动画常量。
 */
export default function ResourceProjectPanel(props: {
  item: ModItem;
  project: ResourceProject | null;
  loading: boolean;
  error: AppError | null;
  onRetry: () => void;
}) {
  const { item, project, loading, error, onRetry } = props;
  const [open, setOpen] = useState(false);

  const description = project?.description || item.description;
  const downloads = project?.downloads ?? item.downloads;
  const updated = project?.updated ?? item.updated;

  return (
    <Paper
      variant="outlined"
      sx={{ flexShrink: 0, borderRadius: 1, overflow: "hidden", mb: 1 }}
    >
      <ButtonBase
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
        sx={{
          width: "100%",
          display: "flex",
          alignItems: "flex-start",
          justifyContent: "flex-start",
          textAlign: "left",
          gap: 1.25,
          px: 1.75,
          py: 1.25,
          transition: "background-color 160ms ease",
          "&:hover": { bgcolor: "action.hover" },
        }}
      >
        <Box
          sx={{
            width: 32,
            height: 32,
            flexShrink: 0,
            borderRadius: 1,
            overflow: "hidden",
            bgcolor: "action.hover",
            display: "grid",
            placeItems: "center",
            mt: 0.25,
          }}
        >
          {project?.iconUrl || item.iconUrl ? (
            <Box
              component="img"
              src={project?.iconUrl ?? item.iconUrl ?? ""}
              alt=""
              loading="lazy"
              sx={{ width: "100%", height: "100%", objectFit: "cover" }}
            />
          ) : (
            <Typography sx={{ fontSize: 13, fontWeight: 700, color: "text.secondary" }}>
              {item.title.slice(0, 1).toUpperCase()}
            </Typography>
          )}
        </Box>

        <Box sx={{ flex: 1, minWidth: 0 }}>
          <Stack direction="row" spacing={0.75} sx={{ alignItems: "center", minWidth: 0 }}>
            <Typography noWrap sx={{ fontSize: 13.5, fontWeight: 600 }}>
              {item.title}
            </Typography>
            {(project?.author || item.author) && (
              <Typography noWrap sx={{ fontSize: 11.5, color: "text.secondary", flexShrink: 0 }}>
                {project?.author || item.author}
              </Typography>
            )}
          </Stack>
          <Typography
            sx={{
              fontSize: 11.5,
              color: "text.secondary",
              mt: 0.25,
              whiteSpace: "pre-wrap",
              display: "-webkit-box",
              WebkitBoxOrient: "vertical",
              WebkitLineClamp: open ? undefined : 2,
              overflow: "hidden",
            }}
          >
            {description || "没有简介"}
          </Typography>
          <Typography
            sx={{
              fontSize: 11,
              color: "text.secondary",
              mt: 0.5,
              fontVariantNumeric: "tabular-nums",
            }}
          >
            {formatCount(downloads)} 次下载 · {formatDate(updated)}
          </Typography>
        </Box>

        <ExpandMoreIcon
          sx={{
            fontSize: 18,
            color: "text.secondary",
            flexShrink: 0,
            mt: 0.5,
            transform: open ? "rotate(180deg)" : "none",
            transition: `transform ${CARD_MOTION.collapse}ms ${EASE.outFluent}`,
          }}
        />
      </ButtonBase>

      <Reveal
        open={open}
        innerSx={{
          maxHeight: "38vh",
          overflowY: "auto",
          scrollbarGutter: "stable",
        }}
      >
        <Box sx={{ px: 1.75, pb: 1.5 }}>
          {loading && !project ? (
            <Box sx={{ display: "grid", placeItems: "center", py: 2 }}>
              <Loading />
            </Box>
          ) : error ? (
            <ErrorState variant="inline" error={error} onRetry={onRetry} sx={{ mt: 0.5 }} />
          ) : project ? (
            <ProjectDetails project={project} />
          ) : (
            <Typography sx={{ fontSize: 12, color: "text.secondary", py: 1 }}>
              没有更多详情
            </Typography>
          )}
        </Box>
      </Reveal>
    </Paper>
  );
}

/** 展开后的正文：元数据、截图、Markdown、外链。 */
function ProjectDetails(props: { project: ResourceProject }) {
  const { project } = props;
  const meta: string[] = [];
  if (project.clientSide) meta.push(`客户端 ${sideLabel(project.clientSide)}`);
  if (project.serverSide) meta.push(`服务端 ${sideLabel(project.serverSide)}`);
  if (project.license) meta.push(project.license);

  return (
    <Stack spacing={1} sx={{ pt: 0.5 }}>
      <Stack direction="row" sx={{ flexWrap: "wrap", gap: 0.5 }}>
        {/* 只显示认得的加载器：资源包项目的 `loaders` 是 `["minecraft"]`，
            原样渲染就是一个没信息量的英文小写胶囊 */}
        {platformLoaderLabels(project.loaders).map((loader) => (
          <Chip
            key={`loader:${loader}`}
            size="small"
            label={loader}
            sx={{ height: 18, fontSize: 10 }}
          />
        ))}
        {/* `minecraft` 是 Modrinth 给所有项目都挂的分类，没有信息量 */}
        {project.categories
          .filter((category) => category.toLowerCase() !== "minecraft")
          .map((category) => (
            <Chip
              key={`category:${category}`}
              size="small"
              variant="outlined"
              label={category}
              sx={{ height: 18, fontSize: 10 }}
            />
          ))}
        {meta.map((value) => (
          <Chip
            key={`meta:${value}`}
            size="small"
            variant="outlined"
            label={value}
            sx={{ height: 18, fontSize: 10 }}
          />
        ))}
      </Stack>

      {project.gallery.length > 0 && (
        <Stack
          direction="row"
          spacing={1}
          sx={{
            overflowX: "auto",
            scrollbarWidth: "none",
            "&::-webkit-scrollbar": { display: "none" },
            pb: 0.25,
          }}
        >
          {project.gallery.map((image) => (
            <ButtonBase
              key={image.url}
              disableRipple
              onClick={() => openExternal(image.url)}
              sx={{
                width: 150,
                height: 92,
                flexShrink: 0,
                borderRadius: 1,
                overflow: "hidden",
                bgcolor: "action.hover",
              }}
            >
              <Box
                component="img"
                src={image.url}
                alt={image.title ?? ""}
                loading="lazy"
                sx={{ width: "100%", height: "100%", objectFit: "cover" }}
              />
            </ButtonBase>
          ))}
        </Stack>
      )}

      {project.body && (
        <Box sx={markdownSx}>
          <ReactMarkdown
            remarkPlugins={[remarkGfm]}
            // Modrinth 的 body 混着大量原始 HTML（<p>/<img>/<strong>/<span>）：
            // 先用 rehype-raw 解析，再用 rehype-sanitize 白名单清洗（防 XSS）。
            rehypePlugins={[rehypeRaw, rehypeSanitize]}
            components={{
              a: ({ href, children }) => (
                <a
                  href={href}
                  onClick={(event) => {
                    event.preventDefault();
                    if (href) openExternal(href);
                  }}
                >
                  {children}
                </a>
              ),
            }}
          >
            {project.body}
          </ReactMarkdown>
        </Box>
      )}

      {(project.links.length > 0 || project.url) && (
        <Stack direction="row" sx={{ flexWrap: "wrap", gap: 1 }}>
          {project.links.map((link) => (
            <Button
              key={`${link.label}:${link.url}`}
              size="small"
              variant="outlined"
              onClick={() => openExternal(link.url)}
            >
              {link.label}
            </Button>
          ))}
          {project.url && (
            <Button
              size="small"
              onClick={() => openExternal(project.url)}
            >
              项目主页
            </Button>
          )}
        </Stack>
      )}
    </Stack>
  );
}

/** 客户端 / 服务端字段的文案（Modrinth 用 required/optional/unsupported）。 */
function sideLabel(value: string): string {
  switch (value) {
    case "required":
      return "必需";
    case "optional":
      return "可选";
    case "unsupported":
      return "不支持";
    default:
      return value;
  }
}

/**
 * Markdown 渲染样式。react-markdown 默认不接受原始 HTML，
 * 这里只给排版，链接点击统一走 `tauri-plugin-opener`。
 */
const markdownSx = {
  fontSize: 12.5,
  lineHeight: 1.75,
  wordBreak: "break-word",
  "& h1": { fontSize: 16.5, fontWeight: 700, mt: 1.5, mb: 0.75 },
  "& h2": { fontSize: 15, fontWeight: 700, mt: 1.5, mb: 0.5 },
  "& h3, & h4, & h5, & h6": { fontSize: 13.5, fontWeight: 700, mt: 1.25, mb: 0.5 },
  "& p": { my: 0.75 },
  "& a": {
    color: (theme: { palette: { brand: { ink: string } } }) => theme.palette.brand.ink,
    textDecoration: "underline",
    cursor: "pointer",
  },
  "& img": { maxWidth: "100%", borderRadius: 1, display: "block", my: 0.75 },
  "& ul, & ol": { pl: 2.5, my: 0.5 },
  "& li": { mb: 0.25 },
  "& code": {
    fontFamily: "Consolas, monospace",
    fontSize: 11.5,
    px: 0.5,
    borderRadius: 0.5,
    bgcolor: (theme: { palette: { text: { primary: string } } }) =>
      alpha(theme.palette.text.primary, 0.06),
  },
  "& pre": {
    bgcolor: (theme: { palette: { text: { primary: string } } }) =>
      alpha(theme.palette.text.primary, 0.06),
    p: 1.25,
    borderRadius: 1,
    overflowX: "auto",
    mb: 1,
  },
  "& blockquote": {
    borderLeft: "3px solid",
    borderColor: "divider",
    pl: 1.5,
    my: 0.75,
    color: "text.secondary",
  },
  "& table": {
    width: "100%",
    borderCollapse: "collapse",
    fontSize: 11.5,
    "& th, & td": { border: "1px solid", borderColor: "divider", px: 0.75, py: 0.5 },
  },
} as const;
