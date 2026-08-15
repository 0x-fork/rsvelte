use rsvelte_core::{CompileOptions, GenerateMode, compile};

fn compile_server(source: &str) {
    compile(
        source,
        CompileOptions {
            generate: GenerateMode::Server,
            ..CompileOptions::default()
        },
    )
    .unwrap();
}

#[test]
fn dollar_prefixed_function_parameter_is_not_a_store_subscription() {
    compile_server(
        r#"<script>
            import { writable } from 'svelte/store';
            const work = writable({ value: 1 });
            function read($work) { return $work.value; }
        </script>
        <p>{$work.value}</p>"#,
    );
}

#[test]
fn escaped_dollar_in_regex_is_not_a_store_subscription() {
    compile_server(
        r#"<script>
            const quote = /"/g;
            const timestamp = '$createdAt';
            const pattern = /^\$createdAt\s*:/;
        </script>"#,
    );
}

#[test]
fn reactive_member_assignment_does_not_create_a_cycle() {
    compile_server(
        r#"<script>
            let data = { size: 255, encrypt: false };
            let size = data.size;
            $: data.size = size;
            $: if (data.encrypt && size < 150) size = 150;
        </script>"#,
    );
}
