use crate::ffi::*;
use anyhow::{anyhow, Context, Result};
use libc::{c_int, c_void};
use std::ffi::{CStr, CString};
use std::time::Duration;
use std::ptr;
use std::sync::{Arc, Mutex};
use std::env;

// Gemma chat template
const GEMMA_TEMPLATE: &str = "<start_of_turn>user\n{prompt}<end_of_turn>\n<start_of_turn>model\n";
// Qwen chat template
const QWEN_TEMPLATE: &str = "<|im_start|>system\nあなたは真面目だけど少しお茶目で優秀なAIです。正確な情報を提供します。必ず日本語で答えてください。<|im_end|><|im_start|>user\n{prompt}<|im_end|>\n<|im_start|>assistant\n";

#[derive(Clone, Copy, Debug)]
pub enum ChatTemplate {
    Gemma,
    Qwen,
}

impl ChatTemplate {
    pub fn from_env() -> Self {
        match env::var("RKLLM_TEMPLATE")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "qwen" => ChatTemplate::Qwen,
            _ => ChatTemplate::Gemma,
        }
    }

    fn apply(&self, prompt: &str) -> String {
        match self {
            ChatTemplate::Gemma => GEMMA_TEMPLATE.replace("{prompt}", prompt),
            ChatTemplate::Qwen => QWEN_TEMPLATE.replace("{prompt}", prompt),
        }
    }
}

pub struct RKLLMConfig {
    pub model_path: String,
    pub max_context_len: i32,
    pub max_new_tokens: i32,
    pub top_k: i32,
    pub top_p: f32,
    pub temperature: f32,
    pub repeat_penalty: f32,
    pub frequency_penalty: f32,
    pub presence_penalty: f32,
    pub mirostat: i32,
    pub mirostat_tau: f32,
    pub mirostat_eta: f32,
    pub skip_special_token: bool,
    pub template: ChatTemplate,
    pub infer_timeout: Duration,
}

impl Default for RKLLMConfig {
    fn default() -> Self {
        Self {
            model_path: String::new(),
            max_context_len: 4096,
            max_new_tokens: 4096,
            top_k: 64,              // default 1
            top_p: 0.95,            // default 0.9
            temperature: 1.0,       // default 0.8
            repeat_penalty: 1.0,    // default 1.1
            frequency_penalty: 0.0,
            presence_penalty: 0.0,  // default 0.0
            mirostat: 0,
            mirostat_tau: 5.0,
            mirostat_eta: 0.1,
            skip_special_token: true,
            template: ChatTemplate::from_env(),
            infer_timeout: infer_timeout_from_env(),
        }
    }
}

struct CallbackContext {
    output_buffer: Vec<u8>,
    is_finished: bool,
    has_error: bool,
}

impl CallbackContext {
    fn new() -> Self {
        Self {
            output_buffer: Vec::new(),
            is_finished: false,
            has_error: false,
        }
    }
}

pub struct RKLLM {
    handle: RKLLMHandleT,
    _model_path: CString,
    _img_start: CString,
    _img_end: CString,
    _img_content: CString,
    template: ChatTemplate,
    infer_timeout: Duration,
}

impl RKLLM {
    pub fn new(config: RKLLMConfig) -> Result<Self> {
        let model_path = CString::new(config.model_path.clone())
            .context("Failed to create CString for model path")?;
        let img_start = CString::new("").context("Failed to create CString for img_start")?;
        let img_end = CString::new("").context("Failed to create CString for img_end")?;
        let img_content = CString::new("").context("Failed to create CString for img_content")?;

        let param = RKLLMParam {
            model_path: model_path.as_ptr(),
            max_context_len: config.max_context_len,
            max_new_tokens: config.max_new_tokens,
            top_k: config.top_k,
            n_keep: -1,                                  // Context keep count (-1 = auto)
            top_p: config.top_p,
            temperature: config.temperature,
            repeat_penalty: config.repeat_penalty,
            frequency_penalty: config.frequency_penalty,
            presence_penalty: config.presence_penalty,
            mirostat: config.mirostat,
            mirostat_tau: config.mirostat_tau,
            mirostat_eta: config.mirostat_eta,
            skip_special_token: if config.skip_special_token { 1 } else { 0 },
            is_async: 0,
            img_start: img_start.as_ptr(),
            img_end: img_end.as_ptr(),
            img_content: img_content.as_ptr(),
            extend_param: RKLLMExtendParam::default(),
        };

        let mut handle: RKLLMHandleT = ptr::null_mut();

        unsafe {
            let ret = rkllm_init(&mut handle, &param, callback_wrapper);
            if ret != 0 {
                return Err(anyhow!("Failed to initialize RKLLM: error code {}", ret));
            }
        }

        Ok(Self {
            handle,
            _model_path: model_path,
            _img_start: img_start,
            _img_end: img_end,
            _img_content: img_content,
            template: config.template,
            infer_timeout: config.infer_timeout,
        })
    }

    pub fn run<F>(&self, prompt: &str, mut _callback: F) -> Result<String>
    where
        F: FnMut(&str),
    {
        // Apply chat template
        let formatted_prompt = self.template.apply(prompt);
        let prompt_cstring =
            CString::new(formatted_prompt).context("Failed to create CString for prompt")?;
        let role_cstring = CString::new("user").context("Failed to create CString for role")?;

        let context = Arc::new(Mutex::new(CallbackContext::new()));
        let context_for_callback = Arc::clone(&context);

        // Store the context pointer in a Box to keep it alive during the call
        let context_ptr = Box::into_raw(Box::new(context_for_callback)) as *mut c_void;

        let input = RKLLMInput {
            role: role_cstring.as_ptr(),
            enable_thinking: 0,
            input_type: RKLLMInputMode::RkllmInputPrompt as c_int,
            input_data: RKLLMInputUnion {
                prompt_input: prompt_cstring.as_ptr(),
            },
        };

        let infer_param = RKLLMInferParam {
            mode: RKLLMInferMode::RkllmInferGenerate,
            lora_params: ptr::null(),
            prompt_cache_params: ptr::null(),
            keep_history: 0,  // Don't keep history between runs
        };

        let ret = unsafe {
            rkllm_run(self.handle, &input, &infer_param, context_ptr)
        };

        // Wait for callback to finish (poll until is_finished or has_error)
        let start_time = std::time::Instant::now();
        let timeout = self.infer_timeout;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(10));
            // Handle poisoned mutex gracefully
            let ctx = match context.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if ctx.is_finished || ctx.has_error {
                break;
            }
            if start_time.elapsed() > timeout {
                eprintln!("\n[Timeout waiting for response]");
                break;
            }
        }

        // Reclaim the Box AFTER callbacks complete
        unsafe {
            let _context_box = Box::from_raw(context_ptr as *mut Arc<Mutex<CallbackContext>>);
        }

        if ret != 0 {
            return Err(anyhow!("Failed to run RKLLM inference: error code {}", ret));
        }

        // Handle poisoned mutex gracefully
        let ctx = match context.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        if ctx.has_error {
            return Err(anyhow!("Error occurred during inference"));
        }

        // 収集した応答テキストを返す
        let output = String::from_utf8_lossy(&ctx.output_buffer).to_string();

        Ok(output)
    }
}

fn infer_timeout_from_env() -> Duration {
    let secs = env::var("RKLLM_INFER_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(120);
    Duration::from_secs(secs)
}

impl Drop for RKLLM {
    fn drop(&mut self) {
        unsafe {
            rkllm_destroy(self.handle);
        }
    }
}

unsafe extern "C" fn callback_wrapper(
    result: *mut RKLLMResult,
    userdata: *mut c_void,
    state: LLMCallState,
) -> c_int {
    // Catch any panics to prevent unwinding across FFI boundary
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        unsafe { callback_impl(result, userdata, state) }
    })) {
        Ok(ret) => ret,
        Err(_) => -1, // Return error on panic
    }
}

unsafe fn callback_impl(
    result: *mut RKLLMResult,
    userdata: *mut c_void,
    state: LLMCallState,
) -> c_int {
    if userdata.is_null() {
        return 0;
    }

    let context_arc = unsafe { &*(userdata as *const Arc<Mutex<CallbackContext>>) };
    let mut context = match context_arc.lock() {
        Ok(ctx) => ctx,
        Err(poisoned) => {
            eprintln!("[WARNING] Mutex was poisoned, recovering...");
            poisoned.into_inner()
        }
    };

    match state {
        LLMCallState::RkllmRunFinish => {
            context.is_finished = true;
            print!("\n");
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }
        LLMCallState::RkllmRunError => {
            context.has_error = true;
            eprintln!("\nError occurred during inference");
        }
        LLMCallState::RkllmRunNormal => {
            if result.is_null() {
                return 0;
            }

            let result_ref = unsafe { &*result };

            if result_ref.text.is_null() {
                return 0;
            }

            // logits/perf are provided by RKLLMResult but CLI ではストリーミングテキストのみを利用する。
            // text is a null-terminated C string, use CStr to read it
            match unsafe { CStr::from_ptr(result_ref.text) }.to_str() {
                Ok(text) => {
                    process_text_chunk(&mut context, text);
                }
                Err(e) => {
                    eprintln!("[DEBUG] UTF-8 decode error: {:?}", e);
                }
            }
        }
        _ => {}
    }

    0  // Return 0 on success
}

/// Process a chunk of text - simply buffer and print it
fn process_text_chunk(context: &mut CallbackContext, text: &str) {
    use std::io::Write;

    // Buffer the output
    context.output_buffer.extend_from_slice(text.as_bytes());

    // Print the text immediately
    print!("{}", text);
    let _ = std::io::stdout().flush();
}
