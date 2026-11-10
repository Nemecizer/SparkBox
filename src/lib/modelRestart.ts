// A manual restart can grant a page-cache drop for this attempt without changing the load.
import { confirmAction, confirmChoice, type ConfirmOptions } from "./session";

export const RESTART_WITH_CACHE_DROP = "Restart and allow cache drop";

/** null cancels; false keeps saved consent; true adds consent for this attempt only. */
export async function confirmModelRestart(options: ConfirmOptions & { body?: string }, vllmLoad: boolean): Promise<boolean | null> {
  if (!vllmLoad) return await confirmAction(options) ? false : null;
  const choice = await confirmChoice({
    ...options,
    body: `${options.body ?? ""} Restart uses any page-cache permission saved with this load. “${RESTART_WITH_CACHE_DROP}” also allows a drop on its nodes if needed for this attempt only (sudo). The page cache is file data Linux keeps in memory; no files are deleted, and Linux refills it. This choice is not saved for later restarts or Keep running.`,
    alternativeLabel: RESTART_WITH_CACHE_DROP,
  });
  return choice === null ? null : choice === "alternative";
}
