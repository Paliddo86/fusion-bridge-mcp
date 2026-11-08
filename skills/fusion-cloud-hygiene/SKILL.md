---
name: fusion-cloud-hygiene
description: Keep the Fusion 360 / Autodesk cloud TIDY when the bridge creates files (f3d 3D packages, saved libraries, exports). The cardinal rule - NEVER write loose files into a shared project ROOT folder; always work inside an AI-owned "Adom AI Workspace" folder with a per-task SUBfolder. Covers why (a real incident where 100+ loose f3d files dumped into the shared Adom team root and colleagues complained), how the bridge now enforces it (defaults uploads to a non-root workspace folder, REFUSES a root folderId), and how to clean up AI-created clutter PRECISELY by lineage urn (fusion_cleanup_cloud_files) without ever touching a teammate's file. Read this before any verb that writes to the cloud - fusion_make_3d_package, fusion_build_library_3d, fusion_aps_upload, fusion_save_to_cloud, fusion_aps_create_folder. Trigger words - fusion cloud folder, project root, dumped files in root, clean up cloud files, fusion_cleanup_cloud_files, Adom AI Workspace, aps_upload folder, save to cloud folder, folder hierarchy fusion, f3d clutter, delete cloud file, lineage urn delete, shared project cloud, organize fusion cloud, never write to root.
---

# Fusion cloud folder hygiene (never clutter a shared root)

The Autodesk/Fusion cloud is a **shared team space**. When the bridge uploads f3d 3D packages, saves
libraries, or exports, those files are visible to every member of the hub/project. So keeping a clean
folder hierarchy is not optional - it is courtesy to your colleagues.

## ⛔ The cautionary tale (why this skill exists, 2026-06-29)

`fusion_make_3d_package` defaulted its upload target to the **project's ROOT folder**. Over two days a
138-part-library build dumped **100+ loose f3d files** (`R_0R_0402`, `C_220nF_3d`, `Untitled`, ...)
straight into the **shared Adom team root**, mixed in with everyone's real designs. Colleagues
complained that it was a mess. Every rule below is the fix.

## Rule 1 - NEVER write a loose file to a project ROOT folder

Not an f3d, not a save, not an export. A project root is the team's shared landing area; loose files
there clutter it for everyone. The bridge now **refuses** an upload whose `folderId` is a known root
(`errorCode: refused_root_upload`) and **defaults** every f3d upload to a non-root workspace folder.

## Rule 2 - work inside an AI-owned "Adom AI Workspace", with a per-task subfolder

The bridge auto-creates ONE tidy folder, **`Adom AI Workspace`**, and puts each task in its own
SUBfolder, e.g. `Adom AI Workspace/AdomBasicParts-3D/`. So:

- `fusion_make_3d_package` / `fusion_build_library_3d` with NO `folderId` → land in
  `Adom AI Workspace/<task>` automatically (`build_library_3d` derives `<task>` from the .lbr name; or
  pass `task`). You do not have to browse for a folder.
- If you DO pass a `folderId`, make it a real subfolder - never a root. (Create one with
  `fusion_aps_create_folder {projectId, parentFolderId, name}` first.)
- Different project/task/situation → a different subfolder. Keep it organized.

## Rule 3 - clean up after yourself, PRECISELY (by lineage urn, never by name)

To remove f3d clutter the bridge made, use **`fusion_cleanup_cloud_files {fileIds:[<lineage urn>...]}`**.
It deletes ONLY the exact ids you pass (gotten from `fusion_aps_browse` on the folder → each item's
`.id`), so it **cannot** delete a teammate's file even if names collide. NEVER bulk-delete by name in a
shared folder - a colleague may have a file with the same name (especially generic ones like
`Untitled`). A big list takes minutes (the relay request may time out at ~60s while the deletes
continue server-side - re-browse the folder to confirm the count dropped).

```jsonc
// find my files
fusion_aps_browse { "projectId": "...", "folderId": "<the folder>" }   // items[].id are lineage urns
// delete exactly mine (protect everyone else's)
fusion_cleanup_cloud_files { "fileIds": ["urn:adsk.wipprod:dm.lineage:...", ...], "projectName": "Adom" }
```

## Rule 4 - to RELOCATE a file, MOVE it (preserve the urn) - don't delete + rebuild

If a file is in the wrong folder (e.g. you need to get a 3D `.f3d` out of the root and into the
workspace), **MOVE it** - Autodesk's cloud keeps the file's **lineage urn** across a move, so anything
that references it (a library's `<package3d wip_urn=...>` 3D binding) **still resolves**. Deleting it
and re-uploading mints a NEW urn, which breaks every binding and forces a full rebuild. (Learned the
hard way 2026-06-29: a library's 10 base f3d were deleted during cleanup instead of moved, so the
library's 3D had to be rebuilt + re-bound. Moving them would have kept the library intact.) So:
clean up only the JUNK by urn; **move** the files you still need.

## The discipline, in one line

Pick (or make) a real, non-root, AI-owned subfolder before you write anything to the cloud; clean up
your own files by precise urn when you're done. Treat the shared cloud the way you'd want a colleague
to treat it.
