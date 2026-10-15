// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's Tools (U-05 D20): files from the OS dialog (dw-app's `api/files.rs`).

import type { FilePurpose } from "../api/generated/FilePurpose";
import type { OpenedFile } from "../api/generated/OpenedFile";
import type { Handlers } from "./handlers";

/** What the OS dialogs answer, and what was saved. */
export interface FilesWorld {
  /** The file the next open dialog picks; `null`: the user cancels. */
  open: OpenedFile | null;
  /** The folder a save dialog picks (the suggested name is kept); `null`: the user cancels. */
  saveDir: string | null;
  /** Every dialog shown, in order. */
  dialogs: { purpose: FilePurpose; save: boolean; suggestedName: string | null }[];
  /** Every file written, in order (data base64). */
  saved: { purpose: FilePurpose; path: string; data: string }[];
}

export interface ToolsWorld {
  files: FilesWorld;
}

export function newToolsWorld(): ToolsWorld {
  return { files: { open: null, saveDir: "/home/user/Documents", dialogs: [], saved: [] } };
}

const savePath = (files: FilesWorld, name: string | null) => (files.saveDir === null ? null : `${files.saveDir}/${name || "Untitled"}`);

export const toolsHandlers: Handlers = {
  open_file: ({ purpose }, { world }) => {
    const { files } = world.tools;
    files.dialogs.push({ purpose, save: false, suggestedName: null });
    return files.open;
  },
  save_file_as: ({ purpose, suggestedName, data }, { world }) => {
    const { files } = world.tools;
    files.dialogs.push({ purpose, save: true, suggestedName });
    const path = savePath(files, suggestedName);
    if (path !== null) files.saved.push({ purpose, path, data });
    return path;
  },
  choose_file_path: ({ purpose, save, suggestedName }, { world }) => {
    const { files } = world.tools;
    files.dialogs.push({ purpose, save, suggestedName });
    return save ? savePath(files, suggestedName) : (files.open && `${files.saveDir ?? "/home/user"}/${files.open.name}`);
  },
};
