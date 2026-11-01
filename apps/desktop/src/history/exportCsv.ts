// Activity ▸ Export CSV (QT-093, 0.1 release plan): the history the filters select, as dash-qt's
// CSV, through the OS's save dialog. The engine writes the bytes (`export_history_csv`); the
// renderer picks the unit, the timezone and the file name.

import type { Backend } from "../api/backend";
import type { CsvUnitName } from "../api/generated/CsvUnitName";
import type { HistoryRequest } from "../api/generated/HistoryRequest";
import { saveText } from "../app/files";

export type CsvFilter = Omit<HistoryRequest, "cursor" | "limit">;

/** `Dash Wallet history 2026-11-01.csv`, the local date. */
export function csvFileName(now: Date): string {
  const two = (n: number) => String(n).padStart(2, "0");
  return `Dash Wallet history ${now.getFullYear()}-${two(now.getMonth() + 1)}-${two(now.getDate())}.csv`;
}

/** Seconds east of UTC at `now`, for the CSV's local dates. */
export const utcOffsetSecs = (now: Date): number => -now.getTimezoneOffset() * 60;

/** Exports the records `filter` selects, in its order. The saved path; `null` when the user
 * cancelled the dialog. */
export async function exportHistory(
  b: Pick<Backend, "exportHistoryCsv" | "saveFileAs">,
  walletId: string,
  filter: CsvFilter,
  unit: CsvUnitName,
  now: Date,
): Promise<string | null> {
  const { sort, ...request } = filter;
  const csv = await b.exportHistoryCsv(walletId, request, sort, unit, [], utcOffsetSecs(now));
  return saveText(b, "csv", csvFileName(now), csv);
}
