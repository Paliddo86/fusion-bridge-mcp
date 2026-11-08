# Test fixtures

- `IPC-2581C.xsd`: the IPC-2581 revision C schema, as KiCad carries it in its QA data
  (`qa/data/pcbnew/ipc2581/IPC-2581C.xsd`). `tests/test_routing.py` validates `fusion_export_ipc2581` output
  against it when lxml is installed.
