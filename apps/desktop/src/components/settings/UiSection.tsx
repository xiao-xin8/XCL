import type { AppConfig } from "../../lib/types";
import type { ConfigPatch } from "../../lib/config";
import SettingGroup from "./SettingGroup";
import { SelectRow, SwitchRow } from "./controls";

const START_OPTIONS = [
  { value: "last", label: "记住上次的页面" },
  { value: "launch", label: "启动页" },
  { value: "download", label: "下载页" },
  { value: "setting", label: "设置页" },
] as const;

const CLOSE_OPTIONS = [
  { value: "background", label: "后台运行" },
  { value: "quit", label: "直接退出" },
] as const;

/**
 * 界面：影响「怎么用」的行为，不影响长相。
 *
 * 这两项都下次启动才生效：起始页是启动时读的，窗口位置是窗口显示前恢复的。
 */
export default function UiSection(props: {
  config: AppConfig;
  patch: (patch: ConfigPatch) => void;
}) {
  const { config, patch } = props;
  const ui = config.ui;

  return (
    <>
      <SettingGroup title="启动行为" desc="下次打开启动器时生效。">
        <SelectRow
          title="启动时打开"
          desc="默认记住你上次停在哪，也可以钉死在某一页。"
          value={ui.startPage}
          options={[...START_OPTIONS]}
          onChange={(next) => patch({ ui: { startPage: next } })}
          width={190}
        />

        <SwitchRow
          title="记住窗口大小与位置"
          desc="关掉之后每次都用配置里的默认尺寸（1180×760），居中显示。下次启动生效。"
          checked={ui.rememberWindow}
          onChange={(next) => patch({ ui: { rememberWindow: next } })}
        />

        <SwitchRow
          title="显示「联机」页签"
          desc="关掉之后顶部不再出现联机入口，联机组件不会被删除。下次启动生效。"
          checked={ui.showConnectTab}
          onChange={(next) => patch({ ui: { showConnectTab: next } })}
        />
      </SettingGroup>

      <SettingGroup title="窗口" desc="关掉窗口之后想怎样。">
        <SelectRow
          title="关闭窗口时"
          desc="后台运行会缩到屏幕右下角，游戏和下载都继续跑，点一下图标就能回来。改完立即生效。"
          value={ui.closeAction}
          options={[...CLOSE_OPTIONS]}
          onChange={(next) => patch({ ui: { closeAction: next } })}
          width={150}
        />
      </SettingGroup>
    </>
  );
}
