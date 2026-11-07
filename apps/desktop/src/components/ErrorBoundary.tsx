import { Component, type ErrorInfo, type ReactNode } from "react";
import { Button } from "@mui/material";
import ErrorState from "./ErrorState";

/**
 * 渲染阶段兜底：React 抛异常时整页会白屏，这里换成统一的「页面出错了」。
 *
 * 只兜 UI 渲染错误，网络/业务错误仍然走各自的 ErrorState —— 不要在
 * 这里顺手吞掉异步异常，那样会把「明明失败却显示正常」变成常态。
 */
export default class ErrorBoundary extends Component<
  { children: ReactNode },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("界面渲染异常", error, info);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;

    return (
      <ErrorState
        variant="page"
        title="页面出错了"
        message={error.message}
        action={
          <Button
            variant="contained"
            size="small"
            onClick={() => window.location.reload()}
          >
            重新加载
          </Button>
        }
        sx={{ minHeight: "100vh" }}
      />
    );
  }
}
