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
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    path: string;
    isDir: boolean;
  } | null>(null);

  useEffect(() => {
    setLoading(true);
    invoke<FileNode>("browse_files", { localRoot })
      .then((t) => setTree(t))
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [localRoot]);

  // Close context menu on any click outside.
  useEffect(() => {
    if (!contextMenu) return;
    const handler = () => setContextMenu(null);
    window.addEventListener("click", handler);
    return () => window.removeEventListener("click", handler);
  }, [contextMenu]);

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

  function openInFinder(path: string) {
    invoke("open_in_finder", { path, baseDir: localRoot }).catch((e) =>
      console.error("Failed to open in Finder:", e)
    );
  }

  function openRootInFinder() {
    invoke("open_in_finder", { path: "", baseDir: localRoot }).catch((e) =>
      console.error("Failed to open root in Finder:", e)
    );
  }

  function handleContextMenu(
    e: React.MouseEvent,
    path: string,
    isDir: boolean
  ) {
    e.preventDefault();
    e.stopPropagation();
    setContextMenu({ x: e.clientX, y: e.clientY, path, isDir });
  }

  if (loading)
    return (
      <div style={{ padding: "20px", color: "#888" }}>Loading files...</div>
    );
  if (error)
    return <div style={{ padding: "20px", color: "#dc2626" }}>{error}</div>;

  return (
    <div
      style={{ display: "flex", height: "100%", position: "relative" }}
      onContextMenu={(e) => {
        // Right-click on empty area → open root.
        e.preventDefault();
        openRootInFinder();
      }}
    >
      <div
        style={{
          width: "300px",
          overflow: "auto",
          borderRight: "1px solid #d0d0d0",
          background: "#fafafa",
        }}
      >
        {/* Root path header with Open in Finder button */}
        <div
          style={{
            padding: "8px 12px",
            borderBottom: "1px solid #e0e0e0",
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
          }}
        >
          <div>
            <div
              style={{
                fontWeight: 600,
                fontSize: "12px",
                color: "#666",
                textTransform: "uppercase",
              }}
            >
              Files
            </div>
            <div
              style={{
                fontSize: "11px",
                color: "#999",
                fontFamily: "monospace",
                marginTop: "2px",
                maxWidth: "200px",
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
              }}
              title={localRoot}
            >
              {localRoot}
            </div>
          </div>
          <button
            onClick={openRootInFinder}
            title="Open in Finder"
            style={{
              padding: "4px 8px",
              fontSize: "11px",
              background: "#2563eb",
              color: "white",
              border: "none",
              borderRadius: "4px",
              cursor: "pointer",
              flexShrink: 0,
            }}
          >
            📁 Finder
          </button>
        </div>
        {tree && (
          <TreeLevel
            node={tree}
            depth={0}
            onFileSelect={openFile}
            selected={selectedFile}
            onContext={handleContextMenu}
          />
        )}
      </div>
      <div style={{ flex: 1, overflow: "auto", padding: "12px" }}>
        {selectedFile ? (
          <>
            <div
              style={{
                display: "flex",
                alignItems: "center",
                justifyContent: "space-between",
                marginBottom: "8px",
              }}
            >
              <div
                style={{
                  fontWeight: 600,
                  fontFamily: "monospace",
                  fontSize: "13px",
                }}
              >
                {selectedFile}
              </div>
              <button
                onClick={() => openInFinder(selectedFile)}
                title="Reveal in Finder"
                style={{
                  padding: "2px 8px",
                  fontSize: "11px",
                  background: "#404040",
                  color: "#ccc",
                  border: "none",
                  borderRadius: "4px",
                  cursor: "pointer",
                }}
              >
                Reveal
              </button>
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
          <div style={{ color: "#888" }}>
            Select a file to view its contents.
          </div>
        )}
      </div>

      {/* Context menu */}
      {contextMenu && (
        <div
          style={{
            position: "fixed",
            left: contextMenu.x,
            top: contextMenu.y,
            background: "white",
            border: "1px solid #d0d0d0",
            borderRadius: "6px",
            boxShadow: "0 4px 12px rgba(0,0,0,0.15)",
            zIndex: 1000,
            minWidth: "160px",
            overflow: "hidden",
          }}
          onClick={(e) => e.stopPropagation()}
        >
          <button
            onClick={() => {
              if (contextMenu.isDir) {
                openInFinder(contextMenu.path);
              } else {
                // Reveal the parent directory with the file selected.
                const parentPath = contextMenu.path.includes("/")
                  ? contextMenu.path.substring(
                      0,
                      contextMenu.path.lastIndexOf("/")
                    )
                  : "";
                openInFinder(parentPath);
              }
              setContextMenu(null);
            }}
            style={{
              display: "block",
              width: "100%",
              padding: "8px 16px",
              fontSize: "13px",
              background: "transparent",
              border: "none",
              cursor: "pointer",
              textAlign: "left",
            }}
            onMouseEnter={(e) =>
              (e.currentTarget.style.background = "#f0f0f0")
            }
            onMouseLeave={(e) =>
              (e.currentTarget.style.background = "transparent")
            }
          >
            📁 {contextMenu.isDir ? "Open in Finder" : "Reveal in Finder"}
          </button>
        </div>
      )}
    </div>
  );
}

function TreeLevel({
  node,
  depth,
  onFileSelect,
  selected,
  onContext,
}: {
  node: FileNode;
  depth: number;
  onFileSelect: (path: string) => void;
  selected: string | null;
  onContext: (e: React.MouseEvent, path: string, isDir: boolean) => void;
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
            onContextMenu={(e) => onContext(e, node.path, true)}
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
              onContext={onContext}
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
        onContextMenu={(e) => onContext(e, node.path, false)}
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
      onContextMenu={(e) => onContext(e, node.path, false)}
    >
      🔗 {node.name}
    </div>
  );
}
