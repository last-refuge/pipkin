-- Accounting provenance only; these rows never reconstruct execution authority.
WITH RECURSIVE lineage(goal_session,goal_id,session,run) AS (
 SELECT g.session,g.id,t.session,t.run FROM goals g JOIN cli_agent_turns t
 ON t.session=g.session AND t.request_key LIKE 'goal:'||g.id||':%'
 UNION
 SELECT l.goal_session,l.goal_id,t.session,t.run FROM lineage l
 JOIN cli_tool_bridges b ON b.session=l.session AND b.run=l.run
 JOIN cli_bridge_calls bc ON bc.bridge=b.id AND bc.session=b.session
 JOIN tool_calls n ON n.session=bc.session AND n.call_id=bc.call_id AND n.run=b.run AND n.name=bc.name
 JOIN sessions s ON s.parent=l.session
 JOIN cli_agent_turns t ON t.session=s.id AND t.request_key='child:'||bc.call_id
 WHERE bc.name IN ('subagent','propose_goal_completion')
 UNION
 SELECT l.goal_session,l.goal_id,t.session,t.run FROM lineage l
 JOIN jobs j ON j.session=l.session AND j.run=l.run AND j.origin='model' AND j.wake=1
 JOIN cli_tool_bridges b ON b.session=j.session AND b.run=j.run
 JOIN cli_bridge_calls bc ON bc.bridge=b.id AND bc.session=j.session AND bc.call_id=j.call_id AND bc.name='job_start'
 JOIN tool_calls n ON n.session=j.session AND n.run=j.run AND n.call_id=j.call_id AND n.name=bc.name
 JOIN cli_agent_turns t ON t.session=j.session AND t.request_key='cli-job-wake:'||j.id
 UNION
 SELECT l.goal_session,l.goal_id,t.session,t.run FROM lineage l
 JOIN cli_tool_bridges b ON b.session=l.session AND b.run=l.run
 JOIN cli_bridge_calls bc ON bc.bridge=b.id AND bc.session=b.session AND bc.name='subagent'
 JOIN tool_calls n ON n.session=bc.session AND n.call_id=bc.call_id AND n.run=b.run AND n.name=bc.name
 JOIN cli_agent_turns child ON child.request_key='child:'||bc.call_id
 JOIN subagent_tasks task ON task.child_session=child.session AND task.parent_session=l.session
 JOIN cli_agent_turns t ON t.session=l.session AND t.request_key=
 CASE WHEN task.parent_call=bc.call_id THEN 'cli-child-wake:'||child.session
 ELSE 'cli-child-wake:'||child.session||':'||bc.call_id END
)
