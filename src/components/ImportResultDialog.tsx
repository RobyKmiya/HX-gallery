import type { ImportReport } from "../types";
import { AlertIcon, CloseIcon } from "./Icons";

interface ImportResultDialogProps {
  report: ImportReport;
  onClose: () => void;
}

export default function ImportResultDialog({
  report,
  onClose,
}: ImportResultDialogProps) {
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <section
        className="modal result-modal"
        role="dialog"
        aria-modal="true"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="modal-title-row">
          <div>
            <p className="eyebrow">IMPORT REPORT</p>
            <h2>导入完成，但有文件未处理</h2>
          </div>
          <button className="icon-button" onClick={onClose} aria-label="关闭">
            <CloseIcon />
          </button>
        </div>

        <div className="result-summary">
          <span>
            成功 <strong>{report.imported}</strong>
          </span>
          <span>
            跳过重复 <strong>{report.skipped}</strong>
          </span>
          <span>
            失败 <strong>{report.failed}</strong>
          </span>
        </div>

        <div className="error-list">
          {report.errors.map((item, index) => (
            <div className="error-item" key={`${item.file}-${index}`}>
              <AlertIcon width={14} height={14} />
              <div>
                <strong title={item.file}>{item.file}</strong>
                <span>{item.message}</span>
              </div>
            </div>
          ))}
        </div>

        <div className="modal-actions">
          <button className="btn btn-primary" onClick={onClose}>
            知道了
          </button>
        </div>
      </section>
    </div>
  );
}
