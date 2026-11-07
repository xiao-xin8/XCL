import ResourceListPage from "./ResourceListPage";

/**
 * 下载页 →「Mod」。
 *
 * 列表结构与「资源包」完全一致，所以两页共用 `ResourceListPage`，
 * 这里只交代类型 —— 不要再把列表逻辑抄回来一份。
 */
export default function ModPage() {
  return <ResourceListPage kind="mod" />;
}
