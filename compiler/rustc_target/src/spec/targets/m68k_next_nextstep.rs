use crate::spec::{Cc, LinkerFlavor, Target, TargetOptions};

pub(crate) fn target() -> Target {
    Target {
        llvm_target: "m68k-next-nextstep".into(),
        metadata: crate::spec::TargetMetadata {
            description: Some("M68k NeXTSTEP".into()),
            tier: Some(3),
            host_tools: Some(false),
            std: Some(false),
        },
        pointer_width: 32,
        data_layout: "E-m:e-p:32:32-i64:64-n8:16:32".into(),
        arch: "m68k".into(),
        
        options: TargetOptions {
            endian: crate::spec::Endian::Big,
            c_int_width: "32".into(),
            cpu: "m68040".into(),
            features: "+m68040".into(),
            max_atomic_width: None, // LLVM M68k backend lacks all atomic lowering support
            atomic_cas: false, // Hardware has CAS but LLVM backend cannot lower atomics
            panic_strategy: crate::spec::PanicStrategy::Abort,
            linker_flavor: LinkerFlavor::Unix(Cc::Yes),
            linker: Some("clang".into()),
            pre_link_args: [(
                LinkerFlavor::Unix(Cc::Yes),
                vec![
                    "-target".into(),
                    "m68k-next-nextstep".into(),
                    "-nostdlib".into(),
                ],
            )]
            .into_iter()
            .collect(),
            os: "nextstep".into(),
            env: "".into(),
            vendor: "next".into(),
            has_rpath: false,
            position_independent_executables: false,
            static_position_independent_executables: false,
            relro_level: crate::spec::RelroLevel::None,
            code_model: Some(crate::spec::CodeModel::Large),
            ..Default::default()
        },
    }
}
