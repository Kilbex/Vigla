import { render, screen } from "@testing-library/react";
import { describe, it, expect } from "vitest";
import PlanEnvelopePanel from "../PlanEnvelopePanel";
import type { EnvelopeFit } from "../types";

describe("PlanEnvelopePanel", () => {
  it("renders one row per bound, with the fit class on each", () => {
    const ef: EnvelopeFit = {
      scope: { fit: "within", note: "all under src/" },
      reversibility: { fit: "near_limit", note: "migration" },
      risk: { fit: "exceeds", note: "billing endpoint" },
      quality: { fit: "within", note: "" },
    };
    render(<PlanEnvelopePanel envelopeFit={ef} />);
    expect(screen.getByText(/^scope$/i)).toBeInTheDocument();
    expect(screen.getByText(/^reversibility$/i)).toBeInTheDocument();
    expect(screen.getByText(/^risk$/i)).toBeInTheDocument();
    expect(screen.getByText(/^quality$/i)).toBeInTheDocument();
    expect(screen.getByText(/billing endpoint/i)).toBeInTheDocument();
    const exceedsRow = screen
      .getByText(/billing endpoint/i)
      .closest(".plan-envelope__row");
    expect(exceedsRow).toHaveClass("plan-envelope__row--exceeds");
  });

  it("renders empty notes as no note span (no 'note: undefined' artifact)", () => {
    const ef: EnvelopeFit = {
      scope: { fit: "within", note: "" },
      reversibility: { fit: "within", note: "" },
      risk: { fit: "within", note: "" },
      quality: { fit: "within", note: "" },
    };
    const { container } = render(<PlanEnvelopePanel envelopeFit={ef} />);
    expect(container.querySelectorAll(".plan-envelope__note")).toHaveLength(0);
  });

  it("returns null when envelopeFit is null", () => {
    const { container } = render(<PlanEnvelopePanel envelopeFit={null} />);
    expect(container.firstChild).toBeNull();
  });

  it("returns null when envelopeFit is undefined", () => {
    const { container } = render(<PlanEnvelopePanel envelopeFit={undefined} />);
    expect(container.firstChild).toBeNull();
  });

  it("renders near_limit fit with its own modifier class", () => {
    const ef: EnvelopeFit = {
      scope: { fit: "within", note: "" },
      reversibility: { fit: "near_limit", note: "migration with rollback" },
      risk: { fit: "within", note: "" },
      quality: { fit: "within", note: "" },
    };
    render(<PlanEnvelopePanel envelopeFit={ef} />);
    const nearRow = screen
      .getByText(/migration with rollback/i)
      .closest(".plan-envelope__row");
    expect(nearRow).toHaveClass("plan-envelope__row--near_limit");
  });
});
