@0xab03450310567eee;

interface Peer {
  workspaces @0 () -> (workspaces: List(Workspace));
}

interface Workspace {
  project @0 () -> (project: Project);
}

interface Project {
  trees @0 () -> (trees: List(FsTree));
}

interface FsTree {
  struct Entry {
    union {
      file @0 :File;
      dir @1 :Directory;
    }
  }

  struct File {
    name @0 :Text;
  }

  struct Directory {
    name @0 :Text;
    children @1 :List(Entry);
  }

  root @0 () -> (root: Entry);
}
