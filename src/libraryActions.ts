import { createContext, useContext } from "react";

/**
 * 库级动作桥：把「弹提示 / 刷新库 / 跳到某标签」这三个能力提到标签壳层。
 *
 * 背景（管理员 2026-11-25）：面板要从浮窗改成标签，但标签壳在 LibraryView
 * 外面，拿不到它内部的 setToast / refresh / browseTag。用这份 context 由
 * LibraryView 注入实现、标签壳里的面板消费，迁一个面板接一个即可。
 */
export interface LibraryActions {
  /** 轻提示：右下角 toast。 */
  notify: (message: string, kind?: "info" | "success" | "error") => void;
  /** 重新拉一次库状态（导入/删除/改名后调用）。 */
  refresh: () => Promise<void>;
  /** 跳到某个标签的作品列表。 */
  browseTag: (tagId: number) => void;
}

const noop: LibraryActions = {
  notify: () => {},
  refresh: async () => {},
  browseTag: () => {},
};

export const LibraryActionsContext = createContext<LibraryActions>(noop);

/** 面板里取库级动作；没有 provider 时退化为空实现，不会炸。 */
export function useLibraryActions(): LibraryActions {
  return useContext(LibraryActionsContext);
}
