import { useCallback, useEffect, useMemo, useState } from "react";
import { Box, Button, Chip, Paper, Stack, Typography, alpha } from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import { apiGet } from "../../lib/api";
import { contentConfig } from "../../lib/contentKinds";
import { formatBytes } from "../../lib/format";
import {
  toAppError,
  type AppError,
  type ContentKind,
  type LocalContentView,
  type ModItem,
  type ResourceProject,
} from "../../lib/types";
import { useToast } from "../Toast";
import ResourceProjectPanel from "../ResourceProjectPanel";
import SettingGroup from "../settings/SettingGroup";
import { ActionRow, InfoRow } from "../settings/controls";

/**
 * 单条本地内容的详情页（从内容管理器点卡片进入，标题栏返回）。
 *
 * 列表状态留在 `ContentManagerPage`，这里只渲染选中项的本地元数据；
 * 有来源记录时再去线上拉项目详情（Modrinth / CurseForge 走现有资源接口）。
 */
export default function ContentDetailPage(props: {
  versionId: string;
  kind: ContentKind;
  item: LocalContentView;
}) {
  const { versionId, kind, item } = props;
  const config = contentConfig(kind);
  const onlineKind = config.onlineKind;
  const Icon = config.icon;
  const toast = useToast();
  const origin = item.origin;
  const originSource = origin?.source ?? null;
  const originProjectId = origin?.projectId ?? null;

  const [project, setProject] = useState<ResourceProject | null>(null);
  const [projectLoading, setProjectLoading] = useState(false);
  const [projectError, setProjectError] = useState<AppError | null>(null);

  const loadProject = useCallback(() => {
    if (!onlineKind || !originSource || !originProjectId) return;
    setProjectLoading(true);
    setProjectError(null);
    apiGet<ResourceProject>("resource_project", {
      source: originSource,
      id: originProjectId,
      kind: onlineKind,
    })
      .then((view) => setProject(view))
      .catch((error: unknown) => {
        setProjectError(toAppError(error, "读取线上项目失败"));
      })
      .finally(() => setProjectLoading(false));
  }, [onlineKind, originProjectId, originSource]);

  useEffect(() => {
    loadProject();
  }, [loadProject]);

  const sourceLabel =
    originSource === "curseforge"
      ? "CurseForge"
      : originSource === "modrinth"
        ? "Modrinth"
        : null;

  const panelItem = useMemo<ModItem | null>(() => {
    if (!origin || !onlineKind) return null;
    return {
      source: origin.source,
      id: origin.projectId,
      url: "",
      title: item.name,
      description: item.description ?? "",
      author: item.authors.join(" / "),
      iconUrl: item.icon,
      downloads: 0,
      updated: null,
      loaders: [],
    };
  }, [item, onlineKind, origin]);

  const openFolder = () => {
    void invoke("version_open_folder", { versionId, folder: config.folder }).catch(
      (error: unknown) => {
        toast(toAppError(error, "打开文件夹失败").message, "error");
      },
    );
  };

  const subtitle = (() => {
    if (config.idLabel) {
      const id = item.contentId ?? `未识别 ${config.idLabel}`;
      return item.version ? `${id} · v${item.version}` : id;
    }
    if (config.formatLabel && item.packFormat != null) {
      return `${config.formatLabel} ${item.packFormat}`;
    }
    return item.fileName;
  })();

  return (
    <Box sx={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column", p: 2 }}>
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
        <Paper
          variant="outlined"
          sx={{
            borderRadius: 1,
            p: 3,
            display: "flex",
            alignItems: "center",
            gap: 2,
          }}
        >
          <Box
            sx={{
              width: 64,
              height: 64,
              flexShrink: 0,
              borderRadius: 1,
              overflow: "hidden",
              bgcolor: "action.hover",
              display: "grid",
              placeItems: "center",
            }}
          >
            {item.icon ? (
              <Box
                component="img"
                src={item.icon}
                alt=""
                loading="lazy"
                sx={{ width: "100%", height: "100%", objectFit: "cover" }}
              />
            ) : (
              <Icon sx={{ fontSize: 30, color: "text.secondary" }} />
            )}
          </Box>
          <Box sx={{ flex: 1, minWidth: 0 }}>
            <Typography
              sx={{ fontSize: 20, fontWeight: 700, letterSpacing: -0.3, lineHeight: 1.3 }}
            >
              {item.name}
            </Typography>
            <Typography
              noWrap
              sx={{ fontSize: 12, color: "text.secondary", mt: 0.5 }}
            >
              {subtitle}
            </Typography>
            <Stack direction="row" sx={{ gap: 0.75, mt: 1, flexWrap: "wrap" }}>
              {sourceLabel && (
                <Chip size="small" label={sourceLabel} sx={{ height: 18, fontSize: 10 }} />
              )}
              <Chip
                size="small"
                label={item.enabled ? "已启用" : "已禁用"}
                sx={{ height: 18, fontSize: 10 }}
              />
              {item.duplicate && config.duplicateLabel && (
                <Chip
                  size="small"
                  label={config.duplicateLabel}
                  sx={{
                    height: 18,
                    fontSize: 10,
                    bgcolor: (t) => alpha(t.palette.warning.main, 0.18),
                    color: "warning.main",
                  }}
                />
              )}
            </Stack>
          </Box>
        </Paper>

        <SettingGroup title="基本信息">
          {config.idLabel && (
            <InfoRow title={config.idLabel} value={item.contentId ?? "未识别"} />
          )}
          {(config.idLabel || item.version) && (
            <InfoRow title="版本" value={item.version ?? "未识别"} />
          )}
          {config.formatLabel &&
            (item.packFormat != null || kind !== "shader") && (
              <InfoRow
                title={config.formatLabel}
                value={item.packFormat != null ? String(item.packFormat) : "未识别"}
              />
            )}
          {(item.authors.length > 0 || config.idLabel) && (
            <InfoRow
              title="作者"
              value={item.authors.length > 0 ? item.authors.join(" / ") : "未署名"}
            />
          )}
          <InfoRow title="文件名" value={item.fileName} />
          <InfoRow title="大小" value={formatBytes(item.size)} />
          <InfoRow
            title="状态"
            desc={
              item.enabled
                ? undefined
                : `禁用只改名，文件仍保留在 ${config.folder} 目录`
            }
            value={item.enabled ? "已启用" : "已禁用"}
          />
          <InfoRow title="来源" value={sourceLabel ?? config.externalSource} />
          <ActionRow title="文件位置" desc={`打开这个版本的 ${config.folder} 目录`}>
            <Button size="small" variant="outlined" onClick={openFolder}>
              打开文件夹
            </Button>
          </ActionRow>
        </SettingGroup>

        <SettingGroup title="简介">
          <Box sx={{ px: 2, py: 1.5 }}>
            <Typography
              sx={{
                fontSize: 12.5,
                color: item.description ? "text.primary" : "text.secondary",
                lineHeight: 1.8,
                whiteSpace: "pre-wrap",
              }}
            >
              {item.description || config.descriptionEmpty}
            </Typography>
          </Box>
        </SettingGroup>

        {panelItem ? (
          <ResourceProjectPanel
            item={panelItem}
            project={project}
            loading={projectLoading}
            error={projectError}
            onRetry={loadProject}
            defaultOpen
          />
        ) : onlineKind ? (
          <SettingGroup title="线上信息">
            <InfoRow title="来源" value={config.externalSource} />
            <Box sx={{ px: 2, py: 1.5 }}>
              <Typography sx={{ fontSize: 11.5, color: "text.secondary", lineHeight: 1.7 }}>
                {config.localOnlyHint}
              </Typography>
            </Box>
          </SettingGroup>
        ) : null}
      </Box>
    </Box>
  );
}
