本目录与 resources/remote/ 的真二进制是构建产物，不入库（对齐 cchaven spec-gaps §B2）。
占位文件由 src-tauri/build.rs 在缺失时自动生成（<1024 字节，运行时必被拒绝），
真产物由 node codex/scripts/sync-components/stage.mjs 覆盖。
不要再用 cchaven/scripts/stage-*.sh 往这里手工拷文件。
