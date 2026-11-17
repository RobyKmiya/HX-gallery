import { AlertIcon, FolderIcon, ImportIcon, LogoIcon, StarIcon } from "./Icons";

interface WelcomeProps {
  lastLibraryPath: string | null;
  error: string | null;
  busy: boolean;
  onChoose: () => void;
}

export default function Welcome({
  lastLibraryPath,
  error,
  busy,
  onChoose,
}: WelcomeProps) {
  return (
    <div className="welcome-shell">
      <div className="welcome-glow welcome-glow-a" />
      <div className="welcome-glow welcome-glow-b" />

      <main className="welcome-card">
        <div className="welcome-brand">
          <span className="brand-mark brand-mark-large">
            <LogoIcon width={22} height={22} />
          </span>
          <span>画匣</span>
        </div>

        <p className="welcome-eyebrow">LOCAL ARCHIVE GALLERY</p>
        <h1>
          把散落的压缩包，
          <br />
          收进一座本地画廊。
        </h1>
        <p className="welcome-lead">
          选择或新建一个库目录。画集会以原始压缩包保存在其中，封面、评分和索引都会跟着库一起走。
        </p>

        <div className="welcome-features">
          <div className="welcome-feature">
            <ImportIcon />
            <div>
              <strong>复制入库</strong>
              <span>源文件保留，自动去重</span>
            </div>
          </div>
          <div className="welcome-feature">
            <FolderIcon />
            <div>
              <strong>本地索引</strong>
              <span>不联网，不上传任何文件</span>
            </div>
          </div>
          <div className="welcome-feature">
            <StarIcon />
            <div>
              <strong>封面与评分</strong>
              <span>百分制评分，默认 60 分</span>
            </div>
          </div>
        </div>

        {error && (
          <div className="welcome-alert" role="alert">
            <AlertIcon />
            <span>{error}</span>
          </div>
        )}

        {lastLibraryPath && (
          <div className="welcome-last" title={lastLibraryPath}>
            上次的库：<code>{lastLibraryPath}</code>
          </div>
        )}

        <button
          className="btn btn-primary btn-large welcome-action"
          onClick={onChoose}
          disabled={busy}
        >
          {busy ? <span className="spinner spinner-light" /> : <FolderIcon />}
          {busy ? "正在打开…" : "选择画廊库目录"}
        </button>

        <p className="welcome-foot">
          完全离线 · 压缩包只读 · 元数据保存在库目录的 .gallery 中
        </p>
      </main>
    </div>
  );
}
