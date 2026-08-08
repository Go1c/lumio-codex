interface Project {
  id: string;
  name: string;
  sshHostAlias: string;
  remoteRoot: string;
  localRoot: string;
  tmuxSession: string;
}

export default function ProjectList({
  projects,
  onSelect,
}: {
  projects: Project[];
  onSelect?: (project: Project) => void;
}) {
  return (
    <ul className="project-list">
      {projects.map((p) => (
        <li
          key={p.id}
          className="project-item"
          onClick={() => onSelect?.(p)}
        >
          {p.name}
        </li>
      ))}
    </ul>
  );
}
