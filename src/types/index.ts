export interface Project {
  id: string;
  name: string;
  description: string;
  path: string;
  tags: string[];
  cover_image: string | null;
  open_count: number;
  last_opened_at: string | null;
  starred: boolean;
  created_at: string;
  last_opened_tools: string[];
  workspace_tools: string[];
}

export interface ToolStatus {
  name: string;
  installed: boolean;
  path: string | null;
}

export interface AppSettings {
  scan_directories: string[];
  tool_paths: Record<string, string>;
}

export interface GitRepo {
  path: string;
  name: string;
}

export interface GitStatus {
  branch: string;
  isDirty: boolean;
  ahead: number;
  behind: number;
  lastCommitTime: string | null;
}
