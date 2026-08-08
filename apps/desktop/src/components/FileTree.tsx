import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

interface FileNode {
  type: "directory" | "file" | "symlink";
  name: string;
  path: string;
  children?: FileNode[];
  size?: number;
  target?: string | null;
}

export default function FileTree({ localRoot }: { localRoot: string }) {
  const [tree, setTree] = useState<FileNode | null>(null);
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [fileContent, setFileContent] = useState<string>("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  useEffect(() => {
    setLoading(true);
    invoke<FileNode>("browse_files", { localRoot })
      .then((t) => setTree(t))
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [localRoot]);

  async function openFile(path: string) {
    setSelectedFile(path);
    setFileContent("");
    try {
      const content = await invoke<string>("read_file", {
        path,
        baseDir: localRoot,
      });
      setFileContent(content);
    } catch (e) {
      setFileContent(`Error reading file: ${e}`);
    }
  }

  if (loading) return <div style={{ padding: "20px", color: "#888" }}>Loading files...</div>;
  if (error) return <div style={{ padding: "20px", color: "#dc2626" }}>{error}</div>;

  return (
    <div style={{ display: "flex", height: "100%" }}>
      <div
        style={{
          width: "300px",
          overflow: "auto",
          borderRight: "1px solid #d0d0d0",
          background: "#fafafa",
        }}
      >
        <div style={{ padding: "8px 12px", fontWeight: 600, fontSize: "12px", color: "#666", textTransform: "uppercase" }}>
          Files
        </div>
        {tree && (
          <TreeLevel node={tree} depth={0} onFileSelect={openFile} selected={selectedFile} />
        )}
      </div>
      <div style={{ flex: 1, overflow: "auto", padding: "12px" }}>
        {selectedFile ? (
          <>
            <div style={{ fontWeight: 600, marginBottom: "8px", fontFamily: "monospace" }}>
              {selectedFile}
            </div>
            <pre
              style={{
                fontSize: "13px",
                fontFamily: "Menlo, Monaco, monospace",
                whiteSpace: "pre-wrap",
                background: "#1e1e1e",
                color: "#d4d4d4",
                padding: "12px",
                borderRadius: "6px",
                overflow: "auto",
              }}
            >
              {fileContent}
            </pre>
          </>
        ) : (
          <div style={{ color: "#888" }}>Select a file to view its contents.</div>
        )}
      </div>
    </div>
  );
}

function TreeLevel({
  node,
  depth,
  onFileSelect,
  selected,
}: {
  node: FileNode;
  depth: number;
  onFileSelect: (path: string) => void;
  selected: string | null;
}) {
  const [expanded, setExpanded] = useState(depth < 2);

  if (node.type === "directory") {
    return (
      <div>
        {depth > 0 && (
          <div
            style={{
              paddingLeft: `${depth * 16 + 8}px`,
              padding: "4px 8px",
              cursor: "pointer",
              fontSize: "13px",
              userSelect: "none",
            }}
            onClick={() => setExpanded(!expanded)}
          >
            {expanded ? "📂" : "📁"} {node.name}
          </div>
        )}
        {expanded &&
          node.children?.map((child) => (
            <TreeLevel
              key={child.path}
              node={child}
              depth={depth + 1}
              onFileSelect={onFileSelect}
              selected={selected}
            />
          ))}
      </div>
    );
  }

  if (node.type === "file") {
    return (
      <div
        style={{
          paddingLeft: `${depth * 16 + 8}px`,
          padding: "4px 8px",
          cursor: "pointer",
          fontSize: "13px",
          background: selected === node.path ? "#dbeafe" : "transparent",
        }}
        onClick={() => onFileSelect(node.path)}
      >
        📄 {node.name}
      </div>
    );
  }

  // Symlink
  return (
    <div
      style={{
        paddingLeft: `${depth * 16 + 8}px`,
        padding: "4px 8px",
        fontSize: "13px",
        color: "#666",
      }}
    >
      🔗 {node.name}
    </div>
  );
}
