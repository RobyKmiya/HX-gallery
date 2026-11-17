import type { ImportProgress } from "../types";

interface ImportOverlayProps {
  progress: ImportProgress | null;
}

const phaseLabels: Record<string, string> = {
  start: "准备导入",
  processing: "复制并建立索引",
  processed: "处理中",
  skipped: "跳过重复项",
  done: "收尾中",
};

export default function ImportOverlay({ progress }: ImportOverlayProps) {
  const total = progress?.total ?? 0;
  const current = progress?.current ?? 0;
  const percent = total > 0 ? Math.min(100, (current / total) * 100) : 0;

  return (
    <div className="modal-backdrop">
      <section className="modal import-modal" role="dialog" aria-modal="true">
        <div className="import-modal-head">
          <span className="pulse-mark" />
          <div>
            <p className="eyebrow">IMPORTING</p>
            <h2>正在导入画集</h2>
          </div>
        </div>

        <p className="import-phase">
          {phaseLabels[progress?.phase ?? ""] ?? "处理中"}
          {total > 0 && (
            <span>
              {current} / {total}
            </span>
          )}
        </p>

        <div
          className={`progress-track${total === 0 ? " is-indeterminate" : ""}`}
        >
          <div
            className="progress-fill"
            style={total > 0 ? { width: `${percent}%` } : undefined}
          />
        </div>

        <p className="import-file" title={progress?.fileName}>
          {progress?.fileName || "正在收集压缩包…"}
        </p>

        <div className="import-stats">
          <span>
            已导入 <strong>{progress?.imported ?? 0}</strong>
          </span>
          <span>
            跳过 <strong>{progress?.skipped ?? 0}</strong>
          </span>
          <span>
            失败 <strong>{progress?.failed ?? 0}</strong>
          </span>
        </div>

        <p className="modal-hint">
          压缩包会复制进库目录，源文件不会被修改。请勿关闭窗口。
        </p>
      </section>
    </div>
  );
}
