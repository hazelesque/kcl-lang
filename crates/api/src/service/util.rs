use crate::gpyrpc::ExecProgramArgs;

/// Transform the str with zero value into [`Option<String>`]
#[inline]
pub(crate) fn transform_str_para(para: &str) -> Option<String> {
    if para.is_empty() {
        None
    } else {
        Some(para.to_string())
    }
}

#[inline]
pub(crate) fn transform_exec_para(
    exec_args: &Option<ExecProgramArgs>,
    plugin_agent: u64,
) -> anyhow::Result<kcl_runner::ExecProgramArgs> {
    let mut args = match exec_args {
        Some(exec_args) => {
            let args_json = serde_json::to_string(exec_args)?;
            // Use try_from_json so a malformed args round-trip surfaces
            // as a structured anyhow::Error rather than a process panic.
            // (Should be unreachable here — args_json was just produced
            // by serde_json::to_string on the same type — but Result
            // propagation defends against future schema drift between
            // the protobuf ExecProgramArgs and kcl_runner's struct.)
            kcl_runner::ExecProgramArgs::try_from_json(args_json.as_str())?
        }
        None => kcl_runner::ExecProgramArgs::default(),
    };
    args.plugin_agent = plugin_agent;
    Ok(args)
}
