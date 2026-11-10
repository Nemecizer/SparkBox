// The Help window's way in: light enough for the registry, so the articles, the search and the
// renderer load only when Help first opens.
import { lazy, Suspense } from "react";
import { LoadingState } from "../components/ui";
import type { Params } from "../wm/WindowManager";

const HelpWindow = lazy(() => import("./HelpWindow"));

export function HelpHost({ params }: { params: Params }) {
  return (
    <Suspense fallback={<LoadingState label="Opening Help" />}>
      <HelpWindow params={params} />
    </Suspense>
  );
}
