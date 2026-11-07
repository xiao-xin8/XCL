import { Box } from "@mui/material";
import type { LocalVersion } from "../../lib/types";
import SettingGroup from "../settings/SettingGroup";
import { TextRow } from "../settings/controls";

/**
 * 说明。
 *
 * 版本名 / 类型 / Java / 目录这些「扫出来的事实」都显示在详情卡的正文里，
 * 弹窗这里只放真正能改的东西 —— 同一个信息摆两遍，只会让人怀疑哪份是真的。
 */
export default function InfoSection(props: {
  version: LocalVersion;
  onSaveDescription: (text: string) => void;
}) {
  const { version, onSaveDescription } = props;

  return (
    <>
      <SettingGroup title="说明" desc="给自己留一句话，写在这里比记在脑子里靠谱。">
        <TextRow
          title="版本说明"
          value={version.description ?? ""}
          onChange={onSaveDescription}
          placeholder="比如：家里那个 1.20.1 生存档，装了 200 个 mod"
          multiline
          rows={3}
        />
      </SettingGroup>

      {!version.valid && (
        <SettingGroup title="这个版本有问题" tone="danger">
          <Box sx={{ px: 2, py: 1.25, fontSize: 12.5, lineHeight: 1.7 }}>
            {version.error ?? "版本描述文件读不出来，先回下载页对它点「校验并修复」。"}
          </Box>
        </SettingGroup>
      )}
    </>
  );
}
