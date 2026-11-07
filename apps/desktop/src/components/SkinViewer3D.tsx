import { useEffect, useRef, useState } from "react";
import { SkinViewer, WalkingAnimation } from "skinview3d";
import type { SkinModel } from "../lib/types";
import { SKIN_FADE } from "../motion/tokens";

/**
 * 皮肤 3D 预览（skinview3d 薄壳）。
 *
 * skinview3d 是 vanilla JS（three.js 驱动），这里只做四件事：
 * 建/销毁 viewer、把 base64 皮肤喂进去、跟随容器宽度、不可见时暂停渲染。
 *
 * 皮肤数据走后端 `skin_current` 命令拿 base64（见 `xcl-service::skin`）——
 * 直接拼 dataURL 加载，绕开 WebView2 的 CORS 与 canvas 污染。
 * 拖拽旋转是 skinview3d 内置的 OrbitControls，不用自己接。
 */
export default function SkinViewer3D(props: {
  /** base64 PNG，不带 `data:` 前缀 */
  skin: string;
  /** base64 PNG；没有披风传 null */
  cape?: string | null;
  model: SkinModel;
  height?: number;
  /** 不可见时暂停渲染循环，省电 */
  paused?: boolean;
}) {
  const { skin, cape, model, height = 128, paused = false } = props;
  const boxRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const viewerRef = useRef<SkinViewer | null>(null);
  // 每个 viewer 实例只淡入一次；之后换皮肤 / 换模型直接切换，不再闪一下
  const revealedRef = useRef(false);
  const [revealed, setRevealed] = useState(false);
  const reduceMotion = useRef(
    window.matchMedia("(prefers-reduced-motion: reduce)").matches,
  ).current;

  const reveal = () => {
    if (revealedRef.current) return;
    revealedRef.current = true;

    const box = boxRef.current;
    if (reduceMotion || !box) {
      setRevealed(true);
      return;
    }

    // 必须用 WAAPI，不能用 CSS transition：
    // 皮肤是 dataURL，解码很快，组件插入和 reveal 经常落在同一帧里，
    // 浏览器从没画过一次 opacity:0，CSS 找不到「变化前」的样式就不会补间。
    const fade = box.animate([{ opacity: 0 }, { opacity: 1 }], {
      duration: SKIN_FADE.duration,
      easing: SKIN_FADE.easing,
      fill: "both",
    });
    fade.finished
      .then(() => {
        // 先落到内联样式再取消动画，避免中间帧闪回透明
        box.style.opacity = "1";
        setRevealed(true);
        fade.cancel();
      })
      .catch(() => setRevealed(true));
  };

  // 建 viewer：只在挂载时建一次。 StrictMode 双挂载会建两次，
  // 靠 cleanup 里的 dispose 兜住（WebGL 上下文不会漏）。
  useEffect(() => {
    const canvas = canvasRef.current;
    const box = boxRef.current;
    if (!canvas || !box) return;
    const viewer = new SkinViewer({
      canvas,
      width: box.clientWidth || 220,
      height,
      enableControls: true,
      zoom: 0.72,
    });
    // 3.4.2 起 autoRotate 是实例属性不是构造参数
    viewer.autoRotate = true;
    viewer.autoRotateSpeed = 0.6;
    // 别光站着转：加一个原地行走动画（四肢摆动 + 头部微晃），
    // 与 autoRotate 互不干扰——一个管角色、一个管相机
    viewer.animation = new WalkingAnimation();
    viewerRef.current = viewer;

    const observer = new ResizeObserver(() => {
      viewer.setSize(box.clientWidth, height);
    });
    observer.observe(box);

    return () => {
      observer.disconnect();
      viewer.dispose();
      viewerRef.current = null;
    };
    // height 变化走下面的 setSize，不重建 viewer
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 尺寸变化
  useEffect(() => {
    const box = boxRef.current;
    const viewer = viewerRef.current;
    if (!box || !viewer) return;
    viewer.setSize(box.clientWidth, height);
  }, [height]);

  // 兜底：WebView2 下窗口 resize 时 ResizeObserver 偶发不触发，canvas 会
  // 停在旧尺寸（表现为「预览区域大小不更新」）。窗口 resize 停当后主动
  // 按容器实际尺寸重算一次。
  useEffect(() => {
    let timer: number | undefined;
    const onResize = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        const box = boxRef.current;
        const viewer = viewerRef.current;
        if (box && viewer && box.clientWidth > 0 && box.clientHeight > 0) {
          viewer.setSize(box.clientWidth, box.clientHeight);
        }
      }, 180);
    };
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      window.clearTimeout(timer);
    };
  }, []);

  // 皮肤数据变化 → 重新加载。损坏的图片不炸界面，保持上一次的样子。
  useEffect(() => {
    const viewer = viewerRef.current;
    if (!viewer || !skin) return;
    const skinUrl = `data:image/png;base64,${skin}`;
    const capeUrl = cape ? `data:image/png;base64,${cape}` : null;
    void viewer
      .loadSkin(skinUrl, { model })
      .then(() => {
        // 卸载后 viewerRef 已换人，别再往旧 viewer 上加载
        if (viewerRef.current !== viewer) return;
        // 重载签名只收纯 null，先把 string|null 收窄
        return capeUrl === null
          ? viewer.loadCape(null)
          : viewer.loadCape(capeUrl);
      })
      .then(() => {
        if (viewerRef.current !== viewer) return;
        // 等下一帧再淡入：这一帧画布上已经是新模型，不会先淡入一块空白
        requestAnimationFrame(() => {
          if (viewerRef.current === viewer) reveal();
        });
      })
      .catch(() => {
        // 皮肤文件损坏或加载失败：viewer 内部会保持原状。
        // 首帧仍然要显示，否则预览框会一直空着。
        if (viewerRef.current === viewer) reveal();
      });
    // reveal 只写 ref / state，不需要进依赖
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [skin, cape, model]);

  // 暂停 / 恢复渲染循环
  useEffect(() => {
    const viewer = viewerRef.current;
    if (viewer) viewer.renderPaused = paused;
  }, [paused]);

  return (
    <div
      ref={boxRef}
      style={{
        width: "100%",
        height,
        lineHeight: 0,
        // 淡入由 reveal() 的 WAAPI 动画负责，这里只负责起始/最终状态
        opacity: revealed ? 1 : 0,
      }}
      aria-hidden="true"
    >
      <canvas ref={canvasRef} style={{ width: "100%", height, display: "block" }} />
    </div>
  );
}
