#version 300 es

layout(location = 0) in vec3 a_position;
layout(location = 1) in vec3 a_normal;

layout(location = 2) in vec3 i_center;
layout(location = 3) in vec3 i_scale;
layout(location = 4) in vec4 i_turn;
layout(location = 5) in vec4 i_color;

uniform mat4 u_view_projection;

out vec3 v_normal;
out vec4 v_color;

vec3 turn(vec4 q, vec3 v) {
	return v + 2.0 * cross(q.xyz, cross(q.xyz, v) + q.w * v);
}

void main() {
	vec3 world = i_center + turn(i_turn, a_position * i_scale);
	v_normal = turn(i_turn, a_normal);
	v_color = i_color;
	gl_Position = u_view_projection * vec4(world, 1.0);
}
