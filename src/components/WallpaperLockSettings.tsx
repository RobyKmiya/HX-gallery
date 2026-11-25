import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import type {
  ArchiveCardData,
  LooseImage,
  LooseImageCollection,
  WallpaperLockConfig,
  WallpaperSource,
} from "../types";
import { describeWallpaper, wallpaperPreviewUrl } from "../wallpapers";

const KIND_LABEL: Record<WallpaperSource["kind"], string> = {
  file: "本地",
  archive: "画廊",
  loose: "散图",
};

/**
 * 设置 → 壁纸锁。
 *
 * 配置落在 Rust 的 settings.json（这样安装目录的「关闭壁纸锁.cmd」才救得了急），
 * 这里只做 UI。列表**不自动加载图片**，点某一条才预览一张，避免多图并发解压卡死。
 */
export default function WallpaperLockSettings() {
  const [config, setConfig] = useState<WallpaperLockConfig | null>(null);
  const [busy, setBusy] = useState(false);
  const [hint, setHint] = useState("");
  const [password, setPassword] = useState("");

  const [sourceKind, setSourceKind] = useState<"file" | "archive" | "loose">("file");
  const [archives, setArchives] = useState<ArchiveCardData[]>([]);
  const [archiveId, setArchiveId] = useState<number | "">("");
  const [archivePage, setArchivePage] = useState(0);
  const [looseCollections, setLooseCollections] = useState<LooseImageCollection[]>([]);
  const [looseCollectionId, setLooseCollectionId] = useState<number | "">("");
  const [looseImages, setLooseImages] = useState<LooseImage[]>([]);
  const [looseImageId, setLooseImageId] = useState<number | "">("");

  /** 选中某条壁纸才加载它的预览；selected 是列表下标。 */
  const [selected, setSelected] = useState<number | null>(null);
  const [previewUrl, setPreviewUrl] = useState<string | null>(null);
  const [previewBusy, setPreviewBusy] = useState(false);

  useEffect(() => {
    void (async () => {
      try {
        setConfig(await api.getWallpaperLock());
      } catch (error) {
        setHint(`读取壁纸锁失败：${String(error)}`);
      }
    })();
  }, []);

  // 懒加载：切到「画廊」才拉最近画集，切到「散图」才拉收藏夹。
  useEffect(() => {
    if (sourceKind !== "archive" || archives.length > 0) return;
    void (async () => {
      try {
        const page = await api.queryArchives({
          page: 1,
          pageSize: 60,
          sort: "added_at",
          direction: "desc",
        });
        setArchives(page.items);
      } catch {
        setHint("画集列表读取失败");
      }
    })();
  }, [sourceKind, archives.length]);

  useEffect(() => {
    if (sourceKind !== "loose" || looseCollections.length > 0) return;
    void (async () => {
      try {
        setLooseCollections(await api.looseListCollections());
      } catch {
        setHint("散图收藏夹读取失败");
      }
    })();
  }, [sourceKind, looseCollections.length]);

  useEffect(() => {
    if (looseCollectionId === "") {
      setLooseImages([]);
      return;
    }
    void (async () => {
      try {
        setLooseImages(await api.looseListImages(Number(looseCollectionId)));
      } catch {
        setHint("散图列表读取失败");
      }
    })();
  }, [looseCollectionId]);

  // 预览：只有被选中的那一条才请求，且画廊走缩略图接口。
  useEffect(() => {
    if (selected === null || !config) {
      setPreviewUrl(null);
      return;
    }
    const wall = config.walls[selected];
    if (!wall) {
      setPreviewUrl(null);
      return;
    }
    let cancelled = false;
    setPreviewBusy(true);
    void wallpaperPreviewUrl(wall)
      .then((url) => {
        if (!cancelled) {
          setPreviewUrl(url);
          setPreviewBusy(false);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setPreviewUrl(null);
          setPreviewBusy(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [selected, config]);

  if (!config) {
    return <p className="settings-hint">正在读取壁纸锁…</p>;
  }

  const persist = async (next: WallpaperLockConfig, done = "已保存") => {
    setBusy(true);
    setHint("");
    try {
      setConfig(await api.saveWallpaperLock(next));
      setHint(done);
    } catch (error) {
      setHint(`保存失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const addWalls = (sources: WallpaperSource[]) => {
    if (sources.length === 0) return;
    void persist(
      { ...config, walls: [...config.walls, ...sources] },
      `已添加 ${sources.length} 张壁纸`,
    );
  };

  const removeWall = (index: number) => {
    setSelected((current) => {
      if (current === null) return null;
      if (current === index) return null;
      return current > index ? current - 1 : current;
    });
    void persist(
      { ...config, walls: config.walls.filter((_, current) => current !== index) },
      "已移除",
    );
  };

  const pickLocal = async () => {
    const picked = await open({
      multiple: true,
      filters: [
        { name: "图片", extensions: ["jpg", "jpeg", "png", "webp", "gif", "bmp", "avif"] },
      ],
    });
    if (!picked) return;
    const list = Array.isArray(picked) ? picked : [picked];
    addWalls(
      list
        .filter((path): path is string => typeof path === "string")
        .map((path) => ({ kind: "file" as const, reference: path })),
    );
  };

  const savePassword = async () => {
    setBusy(true);
    setHint("");
    try {
      setConfig(await api.setWallpaperPassword(password));
      setPassword("");
      setHint(
        password.trim()
          ? "密码已更新（下次启动生效）"
          : "密码已清除：无密码，启动时点一下即进",
      );
    } catch (error) {
      setHint(`设置密码失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <h3>壁纸锁</h3>
      <p className="settings-hint">
        开启后，启动画匣先看到壁纸，输密码才进库。定位是防君子不防小人，不做防破解；
        忘了密码就双击安装目录里的「关闭壁纸锁.cmd」。
      </p>

      <label className="settings-row">
        <span>开启壁纸锁</span>
        <select
          value={config.enabled ? "1" : "0"}
          onChange={(event) =>
            void persist(
              { ...config, enabled: event.target.value === "1" },
              event.target.value === "1" ? "壁纸锁已开启" : "壁纸锁已关闭",
            )
          }
        >
          <option value="0">关闭</option>
          <option value="1">开启（启动时先锁屏）</option>
        </select>
      </label>

      <label className="settings-row">
        <span>密码</span>
        <input
          type="password"
          value={password}
          onChange={(event) => setPassword(event.target.value)}
          placeholder="留空保存 = 清除密码"
          className="settings-inline-input"
        />
      </label>
      <div className="settings-row">
        <span />
        <button
          type="button"
          className="btn btn-quiet btn-compact"
          onClick={() => void savePassword()}
          disabled={busy}
        >
          保存密码
        </button>
      </div>

      <label className="settings-row">
        <span>轮换间隔</span>
        <select
          value={String(config.intervalSecs)}
          onChange={(event) =>
            void persist({ ...config, intervalSecs: Number(event.target.value) }, "已更新间隔")
          }
        >
          <option value="0">不轮换</option>
          <option value="10">10 秒</option>
          <option value="30">30 秒</option>
          <option value="60">1 分钟</option>
          <option value="300">5 分钟</option>
          <option value="1800">30 分钟</option>
        </select>
      </label>

      {hint && <p className="settings-hint">{hint}</p>}

      <h3>壁纸（{config.walls.length}）</h3>
      <div className="wallpaper-list">
        {config.walls.map((wall, index) => (
          <div
            className={`wallpaper-row${selected === index ? " is-selected" : ""}`}
            key={`${wall.kind}-${wall.reference}-${index}`}
          >
            <button
              type="button"
              className="wallpaper-row-main"
              onClick={() => setSelected(selected === index ? null : index)}
              title="点一下预览"
            >
              <span className="wallpaper-row-index">{index + 1}</span>
              <span className="wallpaper-row-name" title={wall.reference}>
                {describeWallpaper(wall)}
              </span>
              <span className="wallpaper-row-kind">{KIND_LABEL[wall.kind]}</span>
            </button>
            <button
              type="button"
              className="wallpaper-row-remove"
              onClick={() => removeWall(index)}
              disabled={busy}
              title="移除"
            >
              ×
            </button>
          </div>
        ))}
        {config.walls.length === 0 && <p className="settings-hint">还没有壁纸，先在下面加一张。</p>}
      </div>

      {selected !== null && config.walls[selected] && (
        <div className="wallpaper-preview">
          {previewBusy ? (
            <span className="settings-hint">加载预览…</span>
          ) : previewUrl ? (
            <img src={previewUrl} alt="" />
          ) : (
            <span className="settings-hint">预览失败（文件可能已被移动/删除）</span>
          )}
        </div>
      )}

      <h3>添加壁纸</h3>
      <label className="settings-row">
        <span>来源</span>
        <select
          value={sourceKind}
          onChange={(event) => setSourceKind(event.target.value as typeof sourceKind)}
        >
          <option value="file">本地图片文件</option>
          <option value="archive">画廊封面 / 指定页</option>
          <option value="loose">散图收藏夹里的图</option>
        </select>
      </label>

      {sourceKind === "file" && (
        <div className="settings-row">
          <span />
          <button type="button" className="btn btn-quiet btn-compact" onClick={() => void pickLocal()}>
            选择图片文件…
          </button>
        </div>
      )}

      {sourceKind === "archive" && (
        <>
          <label className="settings-row">
            <span>画廊</span>
            <select
              value={archiveId}
              onChange={(event) => setArchiveId(event.target.value ? Number(event.target.value) : "")}
            >
              <option value="">选择画集</option>
              {archives.map((archive) => (
                <option key={archive.id} value={archive.id}>
                  {archive.title}
                </option>
              ))}
            </select>
          </label>
          <label className="settings-row">
            <span>页码（0 起）</span>
            <input
              type="number"
              min={0}
              value={archivePage}
              onChange={(event) => setArchivePage(Math.max(0, Number(event.target.value) || 0))}
              className="settings-inline-input"
            />
          </label>
          <div className="settings-row">
            <span />
            <button
              type="button"
              className="btn btn-quiet btn-compact"
              disabled={archiveId === ""}
              onClick={() => {
                if (archiveId !== "") addWalls([{ kind: "archive", reference: `${archiveId}:${archivePage}` }]);
              }}
            >
              添加这张
            </button>
          </div>
        </>
      )}

      {sourceKind === "loose" && (
        <>
          <label className="settings-row">
            <span>散图收藏夹</span>
            <select
              value={looseCollectionId}
              onChange={(event) => {
                setLooseCollectionId(event.target.value ? Number(event.target.value) : "");
                setLooseImageId("");
              }}
            >
              <option value="">选择收藏夹</option>
              {looseCollections.map((collection) => (
                <option key={collection.id} value={collection.id}>
                  {collection.name}（{collection.count}）
                </option>
              ))}
            </select>
          </label>
          <label className="settings-row">
            <span>图片</span>
            <select
              value={looseImageId}
              onChange={(event) => setLooseImageId(event.target.value ? Number(event.target.value) : "")}
            >
              <option value="">选择图片</option>
              {looseImages.map((image) => (
                <option key={image.id} value={image.id}>
                  #{image.id}
                  {image.missing ? "（缺失）" : ""}
                </option>
              ))}
            </select>
          </label>
          <div className="settings-row">
            <span />
            <button
              type="button"
              className="btn btn-quiet btn-compact"
              disabled={looseImageId === ""}
              onClick={() => {
                if (looseImageId !== "") addWalls([{ kind: "loose", reference: String(looseImageId) }]);
              }}
            >
              添加这张
            </button>
          </div>
        </>
      )}
    </>
  );
}
