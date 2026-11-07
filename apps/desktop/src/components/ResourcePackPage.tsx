import ResourceListPage from "./ResourceListPage";

/**
 * 下载页 →「资源包」。
 *
 * 与「Mod」完全同一套列表（源胶囊 / 搜索 / 版本 / 排序 / 卡片 / 分页 / 入场动画），
 * 只有平台侧的类型值不同（Modrinth `project_type:resourcepack`、CF `classId=12`），
 * 由后端按 `kind` 翻译。
 *
 * 落点仍然要等详情页接上 `kind`：现在是按 `mods/` 算的，资源包得走 `resourcepacks/`。
 */
export default function ResourcePackPage() {
  return <ResourceListPage kind="resourcepack" />;
}
