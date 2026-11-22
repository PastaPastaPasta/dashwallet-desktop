/** The process groups of every TCP listener on each of `ports`, from one `ss -Hltnp` listing;
 * null for a listener whose owner is not visible (another account's). */
export function snapshotOf(ssOutput: string, ports: number[], groupOf: (pid: string) => number | null) {
  const lines = ssOutput.split("\n");
  return ports.map((port) => ({
    port,
    groups: lines
      .filter((l) => l.split(/\s+/)[3]?.endsWith(`:${port}`))
      .flatMap((l) => {
        const pids = [...l.matchAll(/pid=(\d+)/g)].map((m) => m[1]!);
        return pids.length === 0 ? [null] : pids.map(groupOf);
      }),
  }));
}

/** One snapshot of who listens on each port, judged against the spawned driver's process group:
 * true once every port has a listener and all of them are in `group`, undefined while a port has
 * none yet. A listener outside the group, or one whose owner is not visible (null), throws at once,
 * so the caller never sends a request to a port it has not seen owned. */
export function ownedListeners(snapshot: { port: number; groups: (number | null)[] }[], group: number | undefined): true | undefined {
  for (const { port, groups } of snapshot) {
    const foreign = groups.filter((g) => g === null || g !== group);
    if (group === undefined || foreign.length > 0) throw new Error(`port ${port} has a listener this run did not start (groups ${groups.map(String).join(", ")}, want ${group})`);
  }
  return snapshot.every(({ groups }) => groups.length > 0) ? true : undefined;
}
