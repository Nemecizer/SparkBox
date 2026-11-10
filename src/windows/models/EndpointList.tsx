// Every inference server as a column of cards (EndpointCard), for Endpoints.
import { Cpu } from "lucide-react";
import { EmptyState } from "../../components/ui";
import type { Endpoint } from "../../lib/endpoints";
import { EndpointCard } from "./EndpointCard";

export function EndpointList({ endpoints, empty, selected, onSelect }: {
  endpoints: Endpoint[];
  empty?: { title: string; body: string };
  selected?: string | null;
  onSelect?: (id: string) => void;
}) {
  if (!endpoints.length) {
    return (
      <EmptyState
        icon={Cpu}
        title={empty?.title ?? "No inference servers running"}
        body={empty?.body ?? "SparkBox finds vLLM, SGLang, TensorRT-LLM, NIM, llama.cpp and Ollama on every node by themselves. Load a model from the Library to start one."}
      />
    );
  }
  return (
    <div className="ai-ep-list">
      {endpoints.map((e) => <EndpointCard key={e.id} endpoint={e} selected={selected === e.id} onSelect={onSelect ? () => onSelect(e.id) : undefined} />)}
    </div>
  );
}
