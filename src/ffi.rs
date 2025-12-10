// Allow dead code for FFI bindings that may not be used immediately
#![allow(dead_code)]

use libc::{c_char, c_float, c_int, c_void, size_t};
use std::mem::ManuallyDrop;

// Handle type
pub type RKLLMHandleT = *mut c_void;

// Enums
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LLMCallState {
    RkllmRunNormal = 0,
    RkllmRunWaiting = 1,
    RkllmRunFinish = 2,
    RkllmRunError = 3,
    RkllmRunGetLastHiddenLayer = 4,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RKLLMInputMode {
    RkllmInputPrompt = 0,
    RkllmInputToken = 1,
    RkllmInputEmbed = 2,
    RkllmInputMultimodal = 3,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RKLLMInferMode {
    RkllmInferGenerate = 0,
    RkllmInferGetLastHiddenLayer = 1,
}

// Structures
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RKLLMExtendParam {
    pub base_domain_id: i32,
    pub embed_flash: i8,
    pub enabled_cpus_num: i8,
    pub enabled_cpus_mask: u32,
    pub n_batch: u8,            // Batch size (must be 1-100)
    pub use_cross_attn: i8,
    pub reserved: [u8; 104],
}

impl Default for RKLLMExtendParam {
    fn default() -> Self {
        Self {
            base_domain_id: 0,
            embed_flash: 1,
            enabled_cpus_num: 4,
            enabled_cpus_mask: (1 << 4) | (1 << 5) | (1 << 6) | (1 << 7),
            n_batch: 1,
            use_cross_attn: 0,
            reserved: [0; 104],
        }
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct RKLLMParam {
    pub model_path: *const c_char,
    pub max_context_len: i32,
    pub max_new_tokens: i32,
    pub top_k: i32,
    pub n_keep: i32,                 // NEW in v1.2: Context keep count (comes after top_k)
    pub top_p: c_float,
    pub temperature: c_float,
    pub repeat_penalty: c_float,
    pub frequency_penalty: c_float,
    pub presence_penalty: c_float,
    pub mirostat: i32,
    pub mirostat_tau: c_float,
    pub mirostat_eta: c_float,
    pub skip_special_token: c_int,
    pub is_async: c_int,
    pub img_start: *const c_char,
    pub img_end: *const c_char,
    pub img_content: *const c_char,
    pub extend_param: RKLLMExtendParam,
}

#[repr(C)]
pub struct RKLLMEmbedInput {
    pub embed: *mut c_float,
    pub n_tokens: size_t,
}

#[repr(C)]
pub struct RKLLMTokenInput {
    pub input_ids: *mut i32,
    pub n_tokens: size_t,
}

#[repr(C)]
pub struct RKLLMMultiModelInput {
    pub prompt: *const c_char,
    pub image_embed: *mut c_float,
    pub n_image_tokens: size_t,
}

#[repr(C)]
pub union RKLLMInputUnion {
    pub prompt_input: *const c_char,
    pub embed_input: ManuallyDrop<RKLLMEmbedInput>,
    pub token_input: ManuallyDrop<RKLLMTokenInput>,
    pub multimodal_input: ManuallyDrop<RKLLMMultiModelInput>,
}

#[repr(C)]
pub struct RKLLMInput {
    pub role: *const c_char,
    pub enable_thinking: c_int,     // c_bool maps to c_int in Rust FFI
    pub input_type: c_int,          // RKLLMInputType enum
    pub input_data: RKLLMInputUnion,
}

#[repr(C)]
pub struct RKLLMLoraParam {
    pub lora_adapter_name: *const c_char,
}

#[repr(C)]
pub struct RKLLMPromptCacheParam {
    pub save_prompt_cache: c_int,
    pub prompt_cache_path: *const c_char,
}

#[repr(C)]
pub struct RKLLMInferParam {
    pub mode: RKLLMInferMode,
    pub lora_params: *const RKLLMLoraParam,
    pub prompt_cache_params: *const RKLLMPromptCacheParam,
    pub keep_history: c_int,  // NEW: Keep conversation history
}

#[repr(C)]
pub struct RKLLMResultLastHiddenLayer {
    pub hidden_states: *mut c_float,
    pub embd_size: c_int,
    pub num_tokens: c_int,
}

#[repr(C)]
pub struct RKLLMResultLogits {
    pub logits: *mut c_float,
    pub vocab_size: c_int,
    pub num_tokens: c_int,
}

#[repr(C)]
pub struct RKLLMPerfStat {
    pub prefill_time_ms: c_float,
    pub prefill_tokens: c_int,
    pub generate_time_ms: c_float,
    pub generate_tokens: c_int,
    pub memory_usage_mb: c_float,
}

#[repr(C)]
pub struct RKLLMResult {
    pub text: *const c_char,
    pub token_id: c_int,
    pub last_hidden_layer: RKLLMResultLastHiddenLayer,
    pub logits: RKLLMResultLogits,
    pub perf: RKLLMPerfStat,
}

#[repr(C)]
pub struct RKLLMLoraAdapter {
    pub lora_adapter_path: *const c_char,
    pub lora_adapter_name: *const c_char,
    pub scale: c_float,
}

// Callback function type (returns c_int)
pub type RKLLMCallback = unsafe extern "C" fn(
    result: *mut RKLLMResult,
    userdata: *mut c_void,
    state: LLMCallState,
) -> c_int;

// External functions from librkllmrt.so
extern "C" {
    pub fn rkllm_init(
        handle: *mut RKLLMHandleT,
        param: *const RKLLMParam,
        callback: RKLLMCallback,
    ) -> c_int;

    pub fn rkllm_run(
        handle: RKLLMHandleT,
        input: *const RKLLMInput,
        infer_param: *const RKLLMInferParam,
        userdata: *mut c_void,
    ) -> c_int;

    pub fn rkllm_destroy(handle: RKLLMHandleT) -> c_int;

    pub fn rkllm_load_lora(handle: RKLLMHandleT, adapter: *const RKLLMLoraAdapter) -> c_int;

    pub fn rkllm_load_prompt_cache(handle: RKLLMHandleT, path: *const c_char) -> c_int;
}
