import ResourceListPage from "./ResourceListPage";

/**
 * 下载页 →「光影」。
 *
 * 和「Mod」「资源包」是同一套列表（源胶囊 / 搜索 / 版本 / 排序 / 卡片 / 分页 /
 * 入场动画），平台侧的类型值由后端按 `kind` 翻译（Modrinth `project_type:shader`、
 * CF `classId=6552`）。
 *
 * 光影特有的「必须先有 Iris 才能生效」不在这页处理 —— 后端会给每个文件补上
 * Iris 前置依赖，详情页的依赖卡自然会提示「前往安装」。
 */
export default function ShaderPage() {
  return <ResourceListPage kind="shader" />;
}
