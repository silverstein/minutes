#!/usr/bin/env python3
import json
import sys


def main() -> None:
    if len(sys.argv) != 4:
        print(
            "Usage: smoke_mcpb_handshake.py stdout-path stderr-path exit-code",
            file=sys.stderr,
        )
        raise SystemExit(2)

    out_path, err_path, rc = sys.argv[1], sys.argv[2], sys.argv[3]

    with open(out_path) as f:
        stdout = f.read()
    with open(err_path) as f:
        stderr = f.read()

    responses: dict[int, dict] = {}
    for line in stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        if "id" in msg:
            responses[msg["id"]] = msg

    response = responses.get(0)
    if response is None:
        print("No initialize response on stdout.", file=sys.stderr)
        print(f"--- stdout ({len(stdout)} bytes) ---", file=sys.stderr)
        print(stdout, file=sys.stderr)
        print(f"--- stderr ({len(stderr)} bytes) ---", file=sys.stderr)
        print(stderr, file=sys.stderr)
        print(f"--- exit code: {rc} ---", file=sys.stderr)
        raise SystemExit(1)

    result = response["result"]
    proto = result.get("protocolVersion")
    if proto != "2025-11-25":
        print(f"Expected protocolVersion=2025-11-25, got {proto!r}", file=sys.stderr)
        raise SystemExit(1)

    caps = result.get("capabilities", {})
    if "tools" not in caps or "resources" not in caps:
        print(
            f"Expected tools+resources capabilities, got keys {sorted(caps)}",
            file=sys.stderr,
        )
        raise SystemExit(1)

    ext_ui = caps.get("extensions", {}).get("io.modelcontextprotocol/ui")
    if ext_ui is None:
        print(
            "Expected extensions.io.modelcontextprotocol/ui capability.",
            file=sys.stderr,
        )
        raise SystemExit(1)

    required_response_ids = [1, 2, 3, 4]
    missing = [id_ for id_ in required_response_ids if id_ not in responses]
    if missing:
        print(f"Missing JSON-RPC responses for ids: {missing}", file=sys.stderr)
        print(f"--- stdout ({len(stdout)} bytes) ---", file=sys.stderr)
        print(stdout, file=sys.stderr)
        print(f"--- stderr ({len(stderr)} bytes) ---", file=sys.stderr)
        print(stderr, file=sys.stderr)
        raise SystemExit(1)

    errors = {
        id_: responses[id_]["error"]
        for id_ in required_response_ids
        if "error" in responses[id_]
    }
    if errors:
        print("MCPB smoke requests returned JSON-RPC errors:", file=sys.stderr)
        for id_, error in errors.items():
            print(f"  id={id_}: {error}", file=sys.stderr)
        print(f"--- stderr ({len(stderr)} bytes) ---", file=sys.stderr)
        print(stderr, file=sys.stderr)
        raise SystemExit(1)

    tools = responses[1].get("result", {}).get("tools", [])
    tool_names = {tool.get("name") for tool in tools}
    required_tools = {
        "start_recording",
        "get_status",
        "list_meetings",
        "search_meetings",
        "get_meeting",
        "open_dashboard",
    }
    missing_tools = sorted(required_tools - tool_names)
    if missing_tools:
        print("MCPB tools/list is missing expected tools:", file=sys.stderr)
        for name in missing_tools:
            print(f"  - {name}", file=sys.stderr)
        raise SystemExit(1)

    resources = responses[2].get("result", {}).get("resources", [])
    resource_uris = {resource.get("uri") for resource in resources}
    required_resources = {
        "ui://minutes/dashboard",
        "minutes://meetings/recent",
        "minutes://status",
    }
    missing_resources = sorted(required_resources - resource_uris)
    if missing_resources:
        print("MCPB resources/list is missing expected resources:", file=sys.stderr)
        for uri in missing_resources:
            print(f"  - {uri}", file=sys.stderr)
        raise SystemExit(1)

    dashboard_contents = responses[3].get("result", {}).get("contents", [])
    dashboard = dashboard_contents[0] if dashboard_contents else {}
    dashboard_text = dashboard.get("text", "")
    if dashboard.get("mimeType") != "text/html;profile=mcp-app" or "<html" not in dashboard_text.lower():
        print(
            "Expected ui://minutes/dashboard to return MCP App HTML content.",
            file=sys.stderr,
        )
        raise SystemExit(1)

    status_contents = responses[4].get("result", {}).get("contents", [])
    status = status_contents[0] if status_contents else {}
    if status.get("mimeType") != "application/json":
        print("Expected minutes://status to return application/json.", file=sys.stderr)
        raise SystemExit(1)
    try:
        json.loads(status.get("text", ""))
    except json.JSONDecodeError as exc:
        print(f"Expected minutes://status to return JSON: {exc}", file=sys.stderr)
        raise SystemExit(1)

    server_info = result.get("serverInfo", {})
    print(
        f"MCPB handshake OK: server={server_info.get('name')}@"
        f"{server_info.get('version')} protocol={proto}; "
        f"tools={len(tools)} resources={len(resources)} app_html={len(dashboard_text)} bytes"
    )


if __name__ == "__main__":
    main()
