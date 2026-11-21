import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "../api";
import type { Queue, QueueFillReport, Tag } from "../types";
import { CloseIcon, PlusIcon, RefreshIcon, TrashIcon } from "./Icons";

interface QueuePanelProps {
  tags: Tag[];
  onClose: () => void;
  onNotify: (message: string, kind?: "info" | "success" | "error") => void;
  /** 打开某个待看目录：把它的成员作为列表展示，并回到画廊主视图。 */
  onOpenQueue: (queue: Queue) => void;
  onChanged: () => void;
}

const DEFAULT_SIZE = 10;

/**
 * 待看目录面板。
 *
 * 目录数量不设上限；「生成」按标签权重加权随机挑，「新建空目录」建壳后
 * 回画廊多选批量挑入（按选择顺序排队）。权重 0 的标签会把画集整本排除。
 */
export default function QueuePanel({
  tags,
  onClose,
  onNotify,
  onOpenQueue,
  onChanged,
}: QueuePanelProps) {
  const [queues, setQueues] = useState<Queue[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [showOptions, setShowOptions] = useState(false);
  const [size, setSize] = useState(DEFAULT_SIZE);
  const [includeRead, setIncludeRead] = useState(false);
  const [selectedTagIds, setSelectedTagIds] = useState<number[]>([]);
  const [tagQuery, setTagQuery] = useState("");
  const [queueNameDraft, setQueueNameDraft] = useState("");
  const [createBusy, setCreateBusy] = useState(false);
  const [lastReport, setLastReport] = useState<QueueFillReport | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState<number | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setQueues(await api.listQueues());
    } catch (error) {
      onNotify(`读取待看目录失败：${String(error)}`, "error");
    } finally {
      setLoading(false);
    }
  }, [onNotify]);

  useEffect(() => {
    void load();
  }, [load]);

  // 权重为 0 的标签不参与挑选（它们的作用正是排除），列表里先过滤掉。
  const pickableTags = useMemo(
    () =>
      tags
        .filter((tag) => tag.weight > 0)
        .sort((left, right) => right.count - left.count),
    [tags],
  );

  const filteredTags = useMemo(() => {
    const keyword = tagQuery.trim().toLowerCase();
    if (!keyword) return pickableTags.slice(0, 60);
    return pickableTags
      .filter(
        (tag) =>
          tag.name.toLowerCase().includes(keyword) ||
          (tag.translatedName?.toLowerCase().includes(keyword) ?? false),
      )
      .slice(0, 60);
  }, [pickableTags, tagQuery]);

  const excludedTags = useMemo(
    () => tags.filter((tag) => tag.weight === 0),
    [tags],
  );

  const todayName = useMemo(() => {
    const now = new Date();
    const pad = (value: number) => String(value).padStart(2, "0");
    return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
  }, []);

  async function generate() {
    if (busy) return;
    setBusy(true);
    try {
      const report = await api.fillQueue(todayName, {
        size,
        tagIds: selectedTagIds,
        includeRead,
      });
      setLastReport(report);
      onNotify(
        report.added > 0
          ? `已挑入 ${report.added} 本到「${report.queue.name}」`
          : "没有可挑的画集了，试试放宽条件",
        report.added > 0 ? "success" : "info",
      );
      await load();
      onChanged();
    } catch (error) {
      onNotify(`生成待看目录失败：${String(error)}`, "error");
    } finally {
      setBusy(false);
    }
  }

  async function remove(queue: Queue) {
    if (confirmingDelete !== queue.id) {
      setConfirmingDelete(queue.id);
      window.setTimeout(
        () => setConfirmingDelete((current) => (current === queue.id ? null : current)),
        3200,
      );
      return;
    }
    setConfirmingDelete(null);
    try {
      await api.deleteQueue(queue.id);
      onNotify(`已删除「${queue.name}」`, "success");
      await load();
      onChanged();
    } catch (error) {
      onNotify(`删除失败：${String(error)}`, "error");
    }
  }

  /** 新建一个空目录：回画廊多选画集，按选择顺序排队。 */
  async function createEmpty() {
    const name = queueNameDraft.trim();
    if (!name) {
      onNotify("先给目录起个名字", "error");
      return;
    }
    if (createBusy) return;
    setCreateBusy(true);
    try {
      const queue = await api.createQueue(name);
      setQueueNameDraft("");
      onNotify(`已创建「${queue.name}」，回画廊多选后点「加入待看」挑入`, "success");
      await load();
      onChanged();
    } catch (error) {
      onNotify(`创建失败：${String(error)}`, "error");
    } finally {
      setCreateBusy(false);
    }
  }

  return (
    <div className="modal-backdrop" role="presentation" onClick={onClose}>
      <div
        className="queue-panel"
        role="dialog"
        aria-modal="true"
        aria-label="待看目录"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="queue-panel-head">
          <div>
            <p className="eyebrow">WATCH LIST</p>
            <h2>待看目录</h2>
          </div>
          <div className="queue-panel-head-actions">
            <button
              type="button"
              className="icon-button"
              onClick={() => void load()}
              aria-label="刷新"
              title="刷新"
            >
              <RefreshIcon width={16} height={16} />
            </button>
            <button
              type="button"
              className="icon-button"
              onClick={onClose}
              aria-label="关闭"
            >
              <CloseIcon width={16} height={16} />
            </button>
          </div>
        </header>

        <div className="queue-panel-body">
          <section className="queue-generate">
            <div className="queue-generate-row">
              <div>
                <strong>随机挑 {size} 本</strong>
                <p className="queue-hint">
                  以「{todayName}」为名生成；同名目录会在此基础上追加。
                  目录数量不限，长期保存。
                </p>
              </div>
              <button
                type="button"
                className="btn btn-primary"
                onClick={() => void generate()}
                disabled={busy}
              >
                <PlusIcon width={15} height={15} />
                {busy ? "挑选中…" : "生成"}
              </button>
            </div>

            <div className="queue-generate-row">
              <div>
                <strong>新建空目录</strong>
                <p className="queue-hint">
                  建好后回画廊开「多选」，勾选任意数量的画集，按选择顺序排队。
                </p>
              </div>
              <div style={{ display: "flex", gap: 6 }}>
                <input
                  value={queueNameDraft}
                  onChange={(event) => setQueueNameDraft(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") {
                      event.preventDefault();
                      void createEmpty();
                    }
                  }}
                  placeholder="目录名字…"
                  aria-label="新目录名字"
                  style={{
                    width: 150,
                    padding: "7px 10px",
                    borderRadius: 6,
                    border: "1px solid var(--border-strong)",
                    background: "var(--panel-raised)",
                    color: "var(--text)",
                  }}
                />
                <button
                  type="button"
                  className="btn btn-quiet btn-compact"
                  onClick={() => void createEmpty()}
                  disabled={createBusy}
                >
                  <PlusIcon width={14} height={14} />
                  {createBusy ? "创建中…" : "新建"}
                </button>
              </div>
            </div>

            <button
              type="button"
              className="btn btn-quiet btn-compact"
              onClick={() => setShowOptions((value) => !value)}
            >
              {showOptions ? "收起挑选条件" : "挑选条件"}
            </button>

            {showOptions && (
              <div className="queue-options">
                <label className="queue-option-row">
                  <span>每个目录放几本</span>
                  <input
                    type="number"
                    min={1}
                    max={200}
                    value={size}
                    onChange={(event) =>
                      setSize(
                        Math.min(
                          200,
                          Math.max(1, Number(event.target.value) || DEFAULT_SIZE),
                        ),
                      )
                    }
                  />
                </label>

                <label className="queue-option-row">
                  <input
                    type="checkbox"
                    checked={includeRead}
                    onChange={(event) => setIncludeRead(event.target.checked)}
                  />
                  <span>允许挑已经看过的（默认只挑没看过的）</span>
                </label>

                <div className="queue-option-tags">
                  <div className="queue-option-tags-head">
                    <span>只在选中的标签里挑</span>
                    <span className="queue-hint">
                      不选＝全库；已排除 {excludedTags.length} 个 0 权重标签
                    </span>
                  </div>
                  <input
                    className="queue-tag-search"
                    placeholder="搜索标签…"
                    value={tagQuery}
                    onChange={(event) => setTagQuery(event.target.value)}
                  />
                  {selectedTagIds.length > 0 && (
                    <div className="queue-selected-tags">
                      {selectedTagIds.map((id) => {
                        const tag = tags.find((item) => item.id === id);
                        if (!tag) return null;
                        return (
                          <button
                            key={id}
                            type="button"
                            className="queue-tag-chip"
                            onClick={() =>
                              setSelectedTagIds((current) =>
                                current.filter((value) => value !== id),
                              )
                            }
                            title="点击移除"
                          >
                            {tag.translatedName || tag.name} ×
                          </button>
                        );
                      })}
                    </div>
                  )}
                  <div className="queue-tag-list">
                    {filteredTags.map((tag) => {
                      const active = selectedTagIds.includes(tag.id);
                      return (
                        <button
                          key={tag.id}
                          type="button"
                          className={`queue-tag-pick${active ? " is-active" : ""}`}
                          onClick={() =>
                            setSelectedTagIds((current) =>
                              active
                                ? current.filter((value) => value !== tag.id)
                                : [...current, tag.id],
                            )
                          }
                        >
                          {tag.translatedName || tag.name}
                          <span className="queue-tag-count">{tag.count}</span>
                        </button>
                      );
                    })}
                    {filteredTags.length === 0 && (
                      <p className="queue-hint">没有匹配的标签</p>
                    )}
                  </div>
                </div>
              </div>
            )}

            {lastReport && (
              <div className="queue-report">
                <strong>上次挑选结果</strong>
                <ul>
                  <li>候选池：{lastReport.candidates} 本</li>
                  <li>
                    被 0 权重标签排除：
                    {lastReport.excludedByWeight} 本
                  </li>
                  <li>本次挑入：{lastReport.added} 本</li>
                </ul>
                {lastReport.titles.length > 0 && (
                  <p className="queue-report-titles">
                    {lastReport.titles.join("、")}
                  </p>
                )}
              </div>
            )}
          </section>

          <section className="queue-list">
            <div className="queue-list-head">
              <strong>现有目录</strong>
              <span className="queue-hint">{queues.length} 个</span>
            </div>

            {loading && <p className="queue-hint">读取中…</p>}
            {!loading && queues.length === 0 && (
              <p className="queue-hint">
                还没有待看目录。点上面的「生成」挑一批。
              </p>
            )}

            {queues.map((queue) => {
              const total = queue.count;
              const finished = queue.finished;
              const percent = total > 0 ? Math.round((finished / total) * 100) : 0;
              return (
                <div className="queue-card" key={queue.id}>
                  <div className="queue-card-main">
                    <button
                      type="button"
                      className="queue-card-name"
                      onClick={() => onOpenQueue(queue)}
                      title="在画廊里打开这个目录"
                    >
                      {queue.name}
                    </button>
                    <span className="queue-card-progress-text">
                      {finished} / {total} 已读完
                    </span>
                  </div>
                  <div className="queue-progress-track" aria-hidden="true">
                    <span
                      className="queue-progress-fill"
                      style={{ width: `${percent}%` }}
                    />
                  </div>
                  <div className="queue-card-actions">
                    <button
                      type="button"
                      className="btn btn-quiet btn-compact"
                      onClick={() => onOpenQueue(queue)}
                    >
                      打开
                    </button>
                    <button
                      type="button"
                      className={`btn btn-compact${
                        confirmingDelete === queue.id ? " btn-danger" : " btn-quiet"
                      }`}
                      onClick={() => void remove(queue)}
                    >
                      <TrashIcon width={14} height={14} />
                      {confirmingDelete === queue.id ? "确认删除" : "删除"}
                    </button>
                  </div>
                </div>
              );
            })}
          </section>
        </div>
      </div>
    </div>
  );
}
