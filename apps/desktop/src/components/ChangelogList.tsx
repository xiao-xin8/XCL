import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Box, Chip, Stack, Typography } from "@mui/material";
import { formatDate } from "../lib/format";
import type { AppError, ChangelogRelease } from "../lib/types";
import ErrorState from "./ErrorState";
import Loading from "./Loading";

/**
 * 更新日志列表：设置页的卡片和关于页的弹窗共用这一份。
 *
 * `active` 变真时才请求；后端在本次进程里缓存，所以反复收起/展开不会重复联网。
 * 失败不自动重试 —— 由用户点「重试」，避免网络差时来回打请求。
 */
export default function ChangelogList(props: { active: boolean }) {
  const { active } = props;
  const [releases, setReleases] = useState<ChangelogRelease[] | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    if (!active || releases !== null) return;
    let alive = true;
    setError(null);
    invoke<ChangelogRelease[]>("update_changelog")
      .then((data) => {
        if (alive) setReleases(data);
      })
      .catch((err) => {
        if (alive) setError(err as AppError);
      });
    return () => {
      alive = false;
    };
  }, [active, releases, attempt]);

  const loading = active && releases === null && error === null;

  if (loading) {
    return (
      <Box sx={{ display: "grid", placeItems: "center", py: 3 }}>
        <Loading />
      </Box>
    );
  }
  if (error) {
    return (
      <ErrorState
        variant="banner"
        error={error}
        onRetry={() => {
          setError(null);
          setAttempt((value) => value + 1);
        }}
      />
    );
  }
  if (!releases) return null;
  if (releases.length === 0) {
    return (
      <Typography sx={{ fontSize: 12, color: "text.secondary" }}>
        还没有可显示的更新记录。
      </Typography>
    );
  }

  return (
    <Stack sx={{ gap: 2.5 }}>
      {releases.map((release) => (
        <HistoryEntry key={release.version} release={release} />
      ))}
    </Stack>
  );
}

/** 更新内容的条目列表；「本次更新」和「历史版本」共用一套排版。 */
export function ChangelogLines(props: { lines: string[] }) {
  return (
    <Stack component="ul" sx={{ m: 0, pl: 0, listStyle: "none", gap: 0.75 }}>
      {props.lines.map((line, index) => (
        <Stack
          component="li"
          key={index}
          direction="row"
          sx={{ gap: 1, alignItems: "flex-start" }}
        >
          <Box
            aria-hidden
            sx={{
              width: 5,
              height: 5,
              mt: "8px",
              flexShrink: 0,
              borderRadius: "50%",
              bgcolor: "primary.main",
            }}
          />
          <Typography sx={{ fontSize: 12, lineHeight: 1.75, color: "text.secondary" }}>
            {line}
          </Typography>
        </Stack>
      ))}
    </Stack>
  );
}

function HistoryEntry(props: { release: ChangelogRelease }) {
  const { release } = props;
  // 标题里已经带版本号时不再重复显示一遍
  const title = release.title && !release.title.includes(release.version) ? release.title : "";

  return (
    <Box>
      <Stack direction="row" sx={{ alignItems: "center", gap: 1, flexWrap: "wrap" }}>
        <Typography sx={{ fontSize: 13.5, fontWeight: 600, lineHeight: 1.6 }}>
          {release.version}
        </Typography>
        {release.publishedAt && (
          <Typography sx={{ fontSize: 11.5, color: "text.secondary" }}>
            {formatDate(release.publishedAt)}
          </Typography>
        )}
        {release.force && (
          <Chip size="small" label="必须更新" sx={{ height: 20, fontSize: 10.5 }} />
        )}
      </Stack>
      {title && (
        <Typography sx={{ fontSize: 12.5, lineHeight: 1.65, mt: 0.25 }}>{title}</Typography>
      )}
      {release.notes && (
        <Typography
          sx={{ fontSize: 11.5, color: "text.secondary", lineHeight: 1.7, mt: 0.5 }}
        >
          {release.notes}
        </Typography>
      )}
      {release.changelog.length > 0 && (
        <Box sx={{ mt: 1 }}>
          <ChangelogLines lines={release.changelog} />
        </Box>
      )}
    </Box>
  );
}
