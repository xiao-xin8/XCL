/**
 * 入口页的两张卡：创建房间 / 加入房间。
 *
 * 已登录是谁 + 登出放在两张卡下面一行小字，不做标头 —— 页面不放装饰性标头。
 */
import { useCallback, useState } from "react";
import { Box, Button, Paper, Stack, Typography, alpha } from "@mui/material";
import AddBoxOutlined from "@mui/icons-material/AddBoxOutlined";
import ChevronRightOutlined from "@mui/icons-material/ChevronRightOutlined";
import LoginOutlined from "@mui/icons-material/LoginOutlined";
import { invoke } from "@tauri-apps/api/core";

import type { ConnectAccount } from "../ConnectLoginDialog";

export function EntryCards({
  onPick,
  account,
  onSignedOut,
}: {
  onPick: (view: "create" | "join") => void;
  account: ConnectAccount;
  onSignedOut: () => void;
}) {
  const [busy, setBusy] = useState(false);

  const signOut = useCallback(async () => {
    setBusy(true);
    try {
      await invoke("connect_account_logout");
      onSignedOut();
    } finally {
      setBusy(false);
    }
  }, [onSignedOut]);

  const items = [
    {
      key: "create" as const,
      icon: <AddBoxOutlined sx={{ fontSize: 22 }} />,
      title: "创建房间",
      desc: "你在玩单人游戏，邀请朋友进来一起玩。",
    },
    {
      key: "join" as const,
      icon: <LoginOutlined sx={{ fontSize: 22 }} />,
      title: "加入房间",
      desc: "朋友邀请你，把邀请码粘进来就行。",
    },
  ];

  return (
    <>
      {items.map((item) => (
        <Paper
          key={item.key}
          variant="outlined"
          role="button"
          tabIndex={0}
          onClick={() => onPick(item.key)}
          onKeyDown={(event) => {
            if (event.key === "Enter" || event.key === " ") {
              event.preventDefault();
              onPick(item.key);
            }
          }}
          sx={{
            display: "flex",
            alignItems: "center",
            gap: 1.5,
            px: 2,
            py: 2,
            cursor: "pointer",
            // 和模组卡片保持一致的淡入时长，别一碰就硬切
            transition: "background-color 160ms ease",
            "&:hover": { bgcolor: "action.hover" },
          }}
        >
          <Box
            sx={{
              width: 38,
              height: 38,
              flexShrink: 0,
              borderRadius: 1,
              display: "grid",
              placeItems: "center",
              bgcolor: (t) => alpha(t.palette.primary.main, 0.35),
              color: "text.primary",
            }}
          >
            {item.icon}
          </Box>
          <Box sx={{ flex: 1, minWidth: 0 }}>
            <Typography sx={{ fontSize: 15, fontWeight: 600, lineHeight: 1.5 }}>
              {item.title}
            </Typography>
            <Typography sx={{ fontSize: 12.5, color: "text.secondary", lineHeight: 1.6 }}>
              {item.desc}
            </Typography>
          </Box>
          <ChevronRightOutlined sx={{ fontSize: 20, color: "text.disabled" }} />
        </Paper>
      ))}

      {/* 已登录是谁 + 登出。放在两张卡下面一行小字，不做标头（页面不放装饰性标头） */}
      <Stack
        direction="row"
        sx={{ alignItems: "center", gap: 0.5, px: 0.5, color: "text.secondary" }}
      >
        <Typography sx={{ fontSize: 12 }}>已登录：{account.username}</Typography>
        <Typography sx={{ fontSize: 12 }}>·</Typography>
        <Button
          size="small"
          disabled={busy}
          onClick={() => void signOut()}
          sx={{ fontSize: 12, minWidth: 0, py: 0, color: "text.secondary" }}
        >
          退出登录
        </Button>
      </Stack>
    </>
  );
}
