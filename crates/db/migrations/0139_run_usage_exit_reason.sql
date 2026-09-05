-- +goose Up
-- How the run's turn ended: the runner's typed exit label (Exit::label(),
-- e.g. reviewer_stop, same_error_loop, max_iterations_reached(100/100),
-- text_response(stop_reason=end_turn)). Stored so runs that ended badly can
-- be listed and replayed as fixtures without reading logs. NULL on rows
-- written before this column existed.
ALTER TABLE run_usage ADD COLUMN exit_reason TEXT;

-- +goose Down
ALTER TABLE run_usage DROP COLUMN exit_reason;
