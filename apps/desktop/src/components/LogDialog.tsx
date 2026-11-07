import { useEffect, useMemo, useRef } from "react";
import {
  Box,
  Button,
  Dialog,
  DialogContent,
  DialogTitle,
  Divider,
  IconButton,
  Stack,
  Typography,
  alpha,
} from "@mui/material";
import CloseIcon from "@mui/icons-material/Close";
import type { LaunchLog } from "../lib/types";

/**
 * 游戏日志大对话框（占窗口 85%，和账号管理一样大）。
 *
 * 启动页不再内嵌日志框 —— 日志是后端静默收集的排障材料，平时只会碍眼；
 * 版本信息栏放一个「查看日志」入口，需要时（启动失败、看进度）才弹出来看。
 *
 * 输出策略不变：原样输出 + 错误行高亮，不做崩溃归因。
 */
export default function LogDialog(props: {
  open: boolean;
  onClose: () => void;
  logs: LaunchLog[];
  onClear: () => void;
}) {
  const { open, onClose, logs, onClear } = props;
  const boxRef = useRef<HTMLDivElement>(null);
  // 往上翻看历史时不要强行拽回底部
  const stickToBottom = useRef(true);

  const errorCount = useMemo(() => logs.filter((line) => isErrorLine(line)).length, [logs]);

  // 打开时先回到底部（最新的日志在最下面）
  useEffect(() => {
    if (!open) return;
    stickToBottom.current = true;
    const box = boxRef.current;
    if (box) box.scrollTop = box.scrollHeight;
  }, [open]);

  useEffect(() => {
    const box = boxRef.current;
    if (!box || !stickToBottom.current) return;
    box.scrollTop = box.scrollHeight;
  }, [logs]);

  return (
    <Dialog
      open={open}
      onClose={onClose}
      maxWidth={false}
      sx={{
        "& .MuiDialog-paper": {
          width: "85vw",
          height: "85vh",
          maxWidth: "none",
        },
      }}
    >
      {/* 标题栏用 flex 垂直居中关闭键：absolute + top 魔法数字会让 40px 的
          按钮从矮标题栏里溢出， ripple 圆圈直接压到下面的分隔线上 */}
      <DialogTitle
        sx={{
          display: "flex",
          alignItems: "center",
          fontSize: 15,
          fontWeight: 700,
          py: 1.5,
          pr: 2,
        }}
      >
        游戏日志
        <IconButton
          onClick={onClose}
          aria-label="关闭日志"
          sx={{ ml: "auto", mr: -1, color: "text.secondary", flexShrink: 0 }}
        >
          <CloseIcon />
        </IconButton>
      </DialogTitle>
      <DialogContent dividers sx={{ p: 0, display: "flex", minHeight: 0, overflow: "hidden" }}>
        <Box sx={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column" }}>
          <Stack
            direction="row"
            sx={{ px: 1.5, py: 0.75, alignItems: "center", flexShrink: 0 }}
          >
            <Typography sx={{ fontSize: 12, fontWeight: 600 }}>{logs.length} 行</Typography>
            {errorCount > 0 && (
              <Typography sx={{ fontSize: 11.5, color: "error.main", ml: 1 }}>
                {errorCount} 个错误
              </Typography>
            )}
            <Box sx={{ flex: 1 }} />
            <Button size="small" onClick={onClear} sx={{ fontSize: 11.5 }}>
              清空
            </Button>
          </Stack>
          <Divider />

          <Box
            ref={boxRef}
            onScroll={(event) => {
              const el = event.currentTarget;
              stickToBottom.current =
                el.scrollHeight - el.scrollTop - el.clientHeight < 48;
            }}
            sx={{
              flex: 1,
              minHeight: 0,
              overflow: "auto",
              scrollbarGutter: "stable",
              px: 1.5,
              py: 1,
              bgcolor: (t) => alpha(t.palette.text.primary, 0.025),
              fontFamily: "ui-monospace, Consolas, monospace",
              fontSize: 11.5,
              lineHeight: 1.55,
            }}
          >
            {logs.length === 0 ? (
              <Box sx={{ color: "text.secondary", fontFamily: "inherit" }}>
                启动游戏后，日志会实时出现在这里。
              </Box>
            ) : (
              logs.map((line, index) => (
                <Box
                  key={index}
                  sx={{
                    whiteSpace: "pre-wrap",
                    wordBreak: "break-word",
                    // 只做错误行高亮，不做崩溃归因
                    color: isErrorLine(line) ? "error.main" : "text.primary",
                    opacity: isErrorLine(line) ? 1 : 0.88,
                  }}
                >
                  {line.text || "\u00a0"}
                </Box>
              ))
            )}
          </Box>
        </Box>
      </DialogContent>
    </Dialog>
  );
}

/** 粗略判断一行是不是错误。只看关键字，不猜原因。 */
export function isErrorLine(line: LaunchLog): boolean {
  if (line.stream === "stderr") return true;
  const text = line.text;
  return (
    text.includes("ERROR") ||
    text.includes("Exception") ||
    text.includes("FATAL") ||
    text.includes("Caused by")
  );
}
