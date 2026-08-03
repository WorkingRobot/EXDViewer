#version 300 es

layout(location = 0) in vec3 a_position;
layout(location = 1) in vec2 a_uv;
layout(location = 2) in vec4 a_color;
layout(location = 3) in vec3 i_center;
layout(location = 4) in vec3 i_scale;
layout(location = 5) in vec4 i_turn;
layout(location = 6) in vec4 i_color;

uniform mat4 u_view_projection;
uniform vec3 u_right;
uniform vec3 u_up;
uniform bool u_billboard;

out vec2 v_uv;
out vec4 v_color;

vec3 turn(vec4 q, vec3 v) {
	return v + 2.0 * cross(q.xyz, cross(q.xyz, v) + q.w * v);
}

void main() {
	vec3 local = turn(i_turn, a_position * i_scale);
	// A sprite keeps only what its rotation did in its own plane, and is set into the screen's.
	vec3 world = u_billboard ? i_center + u_right * local.x + u_up * local.y : i_center + local;
	v_uv = a_uv;
	v_color = a_color * i_color;
	gl_Position = u_view_projection * vec4(world, 1.0);
}
