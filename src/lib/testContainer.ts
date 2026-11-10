// Containers › Run test container: SparkBox's own test container on one node (busybox, named
// sparkbox-test-busybox, labelled sparkbox.test=1 and this build's sparkbox.instance), run as the
// cluster user under the fence class container-test (ops/containers.rs). Stop and Remove in
// Containers take it away again.
import { invoke } from "./api";
import { describeError } from "./errors";
import { singleFlight } from "./session";
import { toast } from "./toasts";

export const TEST_CONTAINER = "sparkbox-test-busybox";

/** Why Run test container is greyed, or null when it can run: Live data and one node picked. */
export function testContainerOff(demo: boolean, node: string): string | null {
  if (demo) return "Available with Live data";
  if (!node || node === "all") return "Pick one node under Node";
  return null;
}

/** Starts the test container on `node` and says how it went (a double-click starts it once). */
export function runTestContainer(node: string, nodeName: string): Promise<void> {
  return singleFlight(`test-container:${node}`, async () => {
    try {
      await invoke<string>("container_test_run", { node });
      toast({ kind: "success", title: `Started ${TEST_CONTAINER} on ${nodeName}`, body: "Stop and Remove it here when you're done with it." });
    } catch (e) {
      const d = describeError(e, { action: `Start the test container on ${nodeName}`, node });
      toast({ kind: "error", title: d.title, body: d.message, actions: d.actions.length ? d.actions : undefined });
    }
  });
}
