# Local vLLM for the AI scraper. Runs on the host, not in Compose.
# A 32 GB Mac fits Qwen3-8B only with a short context window.

VLLM_MODEL ?= Qwen/Qwen3-8B
VLLM_HOST ?= 0.0.0.0
VLLM_PORT ?= 8000
VLLM_MAX_MODEL_LEN ?= 4096
VLLM_GPU_MEMORY_UTILIZATION ?= 0.7

.PHONY: vllm

# Foreground server. No API key. App in Docker uses http://host.docker.internal:8000/v1
vllm:
	vllm serve $(VLLM_MODEL) \
		--host $(VLLM_HOST) \
		--port $(VLLM_PORT) \
		--max-model-len $(VLLM_MAX_MODEL_LEN) \
		--gpu-memory-utilization $(VLLM_GPU_MEMORY_UTILIZATION)
