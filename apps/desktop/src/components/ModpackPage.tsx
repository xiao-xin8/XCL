import ResourceListPage from "./ResourceListPage";

/**
 * 下载页 →「整合包」。
 *
 * 列表复用 `ResourceListPage`（搜索 / 版本 / 排序 / 卡片 / 分页 / 动画全一样），
 * 差别只在详情页：整合包装出来是一个新实例，不是往已有版本里塞文件，
 * 所以那边走 `ModpackDetailPage`。
 */
export default function ModpackPage() {
  return <ResourceListPage kind="modpack" />;
}
