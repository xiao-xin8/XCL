import { Dialog, DialogContent, DialogTitle, IconButton } from "@mui/material";
import CloseIcon from "@mui/icons-material/Close";
import ChangelogList from "./ChangelogList";

/** 关于页的更新日志弹窗。列表本身和设置页共用 `ChangelogList`。 */
export default function ChangelogDialog(props: { open: boolean; onClose: () => void }) {
  const { open, onClose } = props;

  return (
    <Dialog
      open={open}
      onClose={onClose}
      maxWidth="sm"
      fullWidth
      sx={{ "& .MuiDialog-paper": { maxHeight: "76vh" } }}
    >
      <DialogTitle
        sx={{ display: "flex", alignItems: "center", fontSize: 15, fontWeight: 700, py: 1.5, pr: 2 }}
      >
        更新日志
        <IconButton
          onClick={onClose}
          aria-label="关闭更新日志"
          sx={{ ml: "auto", mr: -1, color: "text.secondary", flexShrink: 0 }}
        >
          <CloseIcon />
        </IconButton>
      </DialogTitle>
      <DialogContent dividers sx={{ px: 2, py: 1.5 }}>
        <ChangelogList active={open} />
      </DialogContent>
    </Dialog>
  );
}
