import { fireEvent, render, screen } from "@testing-library/react";
import { describe, it, expect, vi } from "vitest";
import PlanMindMap from "../PlanMindMap";

describe("PlanMindMap", () => {
  it("renders inside a ReactFlowProvider without throwing", () => {
    const { container } = render(
      <PlanMindMap
        spec={{ title: "Add OAuth callback", objective: "" }}
        plan={{
          tasks: [
            {
              index: 0,
              title: "A",
              description: "Implement the callback safely",
              depends_on: [],
              criteria_summary: "Callback tests pass",
            },
          ],
          generation: 0,
        }}
      />,
    );
    expect(screen.getByTestId("plan-mind-map")).toBeInTheDocument();
    expect(
      screen.getByRole("group", { name: /mission plan map/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /fit mind map/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /reset mind map zoom/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /download mind map/i }),
    ).toBeInTheDocument();
    const taskNode = container.querySelector('[data-testid="rf__node-task-0"]');
    expect(taskNode).toHaveAttribute("tabindex", "0");
    expect(taskNode?.getAttribute("aria-label")).toMatch(
      /implement the callback safely[\s\S]*callback tests pass/i,
    );
  });

  it("downloads a scalable svg copy of the mind map", async () => {
    const createObjectURL = vi.fn(
      (_blob: Blob | MediaSource) => "blob:mind-map",
    );
    const revokeObjectURL = vi.fn((_url: string) => {});
    Object.defineProperty(URL, "createObjectURL", {
      configurable: true,
      value: createObjectURL,
    });
    Object.defineProperty(URL, "revokeObjectURL", {
      configurable: true,
      value: revokeObjectURL,
    });
    const click = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(() => {});

    render(
      <PlanMindMap
        spec={{ title: "Readable Mind Map", objective: "Zoomable download" }}
        plan={{
          tasks: [
            {
              index: 0,
              title: "Implement export path",
              description: "",
              depends_on: [],
            },
          ],
          generation: 0,
        }}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /download mind map/i }));

    expect(createObjectURL).toHaveBeenCalledTimes(1);
    const blob = createObjectURL.mock.calls[0]?.[0] as Blob | undefined;
    expect(blob).toBeInstanceOf(Blob);
    expect(blob?.type).toBe("image/svg+xml;charset=utf-8");
    const svgText = await blob?.text();
    expect(svgText).toContain("<svg");
    expect(svgText).toContain("Readable Mind Map");
    expect(click).toHaveBeenCalledTimes(1);
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:mind-map");
  });

  it("accepts a tech_stack-only plan without error", () => {
    render(
      <PlanMindMap
        spec={{ title: "T", objective: "" }}
        plan={{
          tasks: [],
          generation: 0,
          tech_stack: [
            { layer: "framework", choice: "Tauri", rationale: "exists", is_new: false },
          ],
        }}
      />,
    );
    expect(screen.getByTestId("plan-mind-map")).toBeInTheDocument();
    expect(screen.getByText("Tech stack")).toBeInTheDocument();
    expect(screen.getByText("Tauri")).toBeInTheDocument();
  });

  it("discloses model content omitted from a bounded preview", () => {
    render(
      <PlanMindMap
        spec={{ title: "T", objective: "" }}
        plan={{
          tasks: [],
          generation: 0,
          tech_stack: Array.from({ length: 25 }, (_, index) => ({
            layer: `layer ${index}`,
            choice: `choice ${index}`,
            rationale: "",
            is_new: false,
          })),
        }}
      />,
    );
    expect(
      screen.getByText(/1 stack item omitted for a responsive preview/i),
    ).toBeInTheDocument();
  });
});
