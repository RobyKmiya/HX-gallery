import { useEffect, useRef, useState } from "react";
import { CloseIcon, PlusIcon } from "./Icons";

export const COLLECTION_NAME_PRESETS = ["全彩", "黑白", "萝莉", "AI"];

interface CollectionNameDialogProps {
  title?: string;
  busy: boolean;
  existingNames: string[];
  onCreate: (name: string) => void;
  onClose: () => void;
}

export default function CollectionNameDialog({
  title = "新建收藏夹",
  busy,
  existingNames,
  onCreate,
  onClose,
}: CollectionNameDialogProps) {
  const [name, setName] = useState("");
  const inputRef = useRef<HTMLInputElement | null>(null);
  const trimmed = name.trim();
  const duplicate =
    trimmed.length > 0 &&
    existingNames.some(
      (existing) => existing.toLocaleLowerCase() === trimmed.toLocaleLowerCase(),
    );

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <form
        className="name-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onClick={(event) => event.stopPropagation()}
        onSubmit={(event) => {
          event.preventDefault();
          if (trimmed && !duplicate && !busy) onCreate(trimmed);
        }}
      >
        <header className="name-dialog-head">
          <div>
            <p className="eyebrow">COLLECTION</p>
            <h2>{title}</h2>
          </div>
          <button
            type="button"
            className="icon-button"
            onClick={onClose}
            aria-label="关闭"
          >
            <CloseIcon width={16} height={16} />
          </button>
        </header>

        <p className="name-dialog-hint">
          给收藏夹起一个名字，例如按题材分类；之后收藏画集时可以直接勾选。
        </p>

        <input
          ref={inputRef}
          value={name}
          maxLength={24}
          placeholder="收藏夹名称"
          disabled={busy}
          onChange={(event) => setName(event.target.value)}
        />

        <div className="name-dialog-presets">
          <span>常用</span>
          {COLLECTION_NAME_PRESETS.map((preset) => (
            <button
              key={preset}
              type="button"
              className={`filter-chip${trimmed === preset ? " is-active" : ""}`}
              onClick={() => setName(preset)}
              disabled={busy}
            >
              {preset}
            </button>
          ))}
        </div>

        {duplicate && !busy && (
          <p className="name-dialog-error" role="alert">
            已存在同名收藏夹，请换一个名称。
          </p>
        )}

        <footer className="name-dialog-foot">
          <span>{trimmed.length} / 24</span>
          <div>
            <button
              type="button"
              className="btn btn-ghost btn-compact"
              onClick={onClose}
              disabled={busy}
            >
              取消
            </button>
            <button
              type="submit"
              className="btn btn-primary btn-compact"
              disabled={busy || !trimmed || duplicate}
            >
              <PlusIcon width={13} height={13} />
              {busy ? "创建中…" : "创建"}
            </button>
          </div>
        </footer>
      </form>
    </div>
  );
}
