interface Project {
  id: string;
  name: string;
  sshHostAlias: string;
  remoteRoot: string;
  localRoot: string;
}

export default function ProjectList({ projects }: { projects: Project[] }) {
  return (
    <ul className="project-list">
      {projects.map((p) => (
        <li key={p.id} className="project-item">
          {p.name}
        </li>
      ))}
    </ul>
  );
}
