import type { Project } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import {
  comparablePath,
  hostBasename,
  hostDirname,
  isAbsoluteHostPath,
  projectAtFolder,
  projectForFile,
  relativeTo,
  samePath,
} from "./hostPaths";

const project = (id: string, rootPath: string | null, archived = false): Project =>
  ({ id, name: id, rootPath, archived }) as unknown as Project;

describe("host paths", () => {
  it("normalizes separators, trailing slashes and Windows case", () => {
    expect(comparablePath("C:\\Code\\App\\")).toBe("c:/code/app");
    expect(comparablePath("C:\\")).toBe("c:/");
    expect(comparablePath("C:")).toBe("c:/");
    expect(comparablePath("/home/Me/app/")).toBe("/home/Me/app");
    expect(comparablePath("/")).toBe("/");
    expect(comparablePath("\\\\server\\share\\x")).toBe("//server/share/x");
  });

  it("compares Windows paths case-insensitively, POSIX paths exactly", () => {
    expect(samePath("C:\\Code\\my-app", "c:/code/MY-APP/")).toBe(true);
    expect(samePath("/home/me/App", "/home/me/app")).toBe(false);
  });

  it("computes paths relative to a root", () => {
    expect(relativeTo("C:\\Code\\app", "C:\\Code\\app\\src\\Main.ts")).toBe("src/Main.ts");
    expect(relativeTo("c:/code/APP", "C:\\Code\\app\\README.md")).toBe("README.md");
    expect(relativeTo("C:\\Code\\app", "C:\\Code\\app")).toBe("");
    expect(relativeTo("C:\\Code\\app", "C:\\Code\\app2\\x.ts")).toBeNull();
    expect(relativeTo("/srv/app", "/srv/app/a/b.txt")).toBe("a/b.txt");
    expect(relativeTo("C:\\", "C:\\x\\y.txt")).toBe("x/y.txt");
  });

  it("splits names and parents in the host's format", () => {
    expect(hostBasename("C:\\Code\\my-app\\")).toBe("my-app");
    expect(hostBasename("/home/me/proj")).toBe("proj");
    expect(hostDirname("C:\\Code\\app\\a.ts")).toBe("C:\\Code\\app");
    expect(hostDirname("C:\\a.ts")).toBe("C:\\");
    expect(hostDirname("/a.ts")).toBe("/");
    expect(hostDirname("/home/me/a.ts")).toBe("/home/me");
  });

  it("recognizes absolute paths", () => {
    expect(isAbsoluteHostPath("C:\\x")).toBe(true);
    expect(isAbsoluteHostPath("/x")).toBe(true);
    expect(isAbsoluteHostPath("\\\\srv\\share")).toBe(true);
    expect(isAbsoluteHostPath("x/y")).toBe(false);
    expect(isAbsoluteHostPath("C:x")).toBe(false);
  });

  it("finds the project at a folder, skipping archived ones", () => {
    const list = [project("a", "C:\\Code\\a", true), project("b", "C:\\Code\\A\\"), project("c", null)];
    expect(projectAtFolder(list, "c:/code/a")?.id).toBe("b");
    expect(projectAtFolder(list, "C:\\Code\\zzz")).toBeUndefined();
  });

  it("maps a file to the deepest containing project", () => {
    const list = [project("outer", "C:\\Code"), project("inner", "C:\\Code\\app"), project("none", null)];
    expect(projectForFile(list, "C:\\Code\\app\\src\\a.ts")).toEqual({ project: list[1], relPath: "src/a.ts" });
    expect(projectForFile(list, "C:\\Code\\other\\b.ts")).toEqual({ project: list[0], relPath: "other/b.ts" });
    expect(projectForFile(list, "D:\\x.ts")).toBeNull();
  });
});
