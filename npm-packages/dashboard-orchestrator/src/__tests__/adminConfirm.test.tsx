import { fireEvent, render, screen } from "@testing-library/react";
import { ConfirmByName } from "../components/admin/ConfirmByName";

describe("ConfirmByName", () => {
  function setup(overrides: Partial<Parameters<typeof ConfirmByName>[0]> = {}) {
    const onConfirm = jest.fn();
    const onCancel = jest.fn();
    render(
      <ConfirmByName
        title="Delete deployment"
        expected="happy-otter-123"
        description="This is not recoverable."
        confirmLabel="Delete permanently"
        onConfirm={onConfirm}
        onCancel={onCancel}
        {...overrides}
      />,
    );
    return {
      onConfirm,
      onCancel,
      input: screen.getByLabelText(/type happy-otter-123 to confirm/i),
      confirm: screen.getByRole("button", { name: /delete permanently/i }),
    };
  }

  it("keeps confirm disabled until the name matches exactly", () => {
    const { input, confirm, onConfirm } = setup();
    expect(confirm).toBeDisabled();

    fireEvent.change(input, { target: { value: "happy-otter" } });
    expect(confirm).toBeDisabled();

    fireEvent.change(input, { target: { value: "happy-otter-123" } });
    expect(confirm).toBeEnabled();

    fireEvent.click(confirm);
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("does not accept a near-miss", () => {
    // Case and whitespace both count: the point of typing it is deliberate
    // effort, and a fleet row's neighbours look almost identical.
    const { input, confirm } = setup();
    fireEvent.change(input, { target: { value: "Happy-Otter-123" } });
    expect(confirm).toBeDisabled();
    fireEvent.change(input, { target: { value: " happy-otter-123 " } });
    expect(confirm).toBeDisabled();
  });

  it("stays disabled while an action is in flight", () => {
    render(
      <ConfirmByName
        title="Delete deployment"
        expected="happy-otter-123"
        description="This is not recoverable."
        confirmLabel="Delete permanently"
        onConfirm={jest.fn()}
        onCancel={jest.fn()}
        busy
      />,
    );
    // While busy the confirm button relabels itself, which is how the
    // operator knows the click landed.
    const confirm = screen.getByRole("button", { name: /working/i });
    fireEvent.change(
      screen.getByLabelText(/type happy-otter-123 to confirm/i),
      { target: { value: "happy-otter-123" } },
    );
    expect(confirm).toBeDisabled();
  });

  it("cancels without confirming", () => {
    const { onCancel, onConfirm } = setup();
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onConfirm).not.toHaveBeenCalled();
  });
});
