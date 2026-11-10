#!/bin/bash
set -e
RANK="$1"
export VLLM_ENGINE_READY_TIMEOUT_S=3600   # model card: engine init is slow
export HF_HUB_OFFLINE=1                    # weights are pre-staged on every node

if [ "$RANK" = "0" ]; then
  NODE_ARGS=(--node-rank 0 --api-key "$API_KEY")
else
  NODE_ARGS=(--node-rank "$RANK" --headless)
fi
EXTRA=()
[ -n "$MOE_BACKEND" ] && EXTRA+=(--moe-backend "$MOE_BACKEND")

exec vllm serve "/models/$MODEL_LOCAL_NAME" \
  --served-model-name "$SERVED_NAME" \
  --host 0.0.0.0 \
  --port "$PORT" \
  --tensor-parallel-size "$NNODES" \
  --max-model-len "$MAX_MODEL_LEN" \
  --max-num-seqs "$MAX_NUM_SEQS" \
  --kv-cache-dtype bfloat16 \
  --gpu-memory-utilization "$GPU_MEM_UTIL" \
  --speculative-config "{\"method\":\"mtp\",\"num_speculative_tokens\":$MTP_TOKENS}" \
  --reasoning-parser deepseek_r1 \
  --tool-call-parser glm47 \
  --enable-auto-tool-choice \
  --nnodes "$NNODES" \
  --master-addr "$HEAD_IP" \
  --master-port "$MASTER_PORT" \
  "${EXTRA[@]}" \
  "${NODE_ARGS[@]}"
