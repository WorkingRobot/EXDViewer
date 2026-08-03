#version 300 es

layout(location = 0) in vec3 a_position;
layout(location = 1) in vec3 a_normal;
layout(location = 3) in vec2 a_uv;
layout(location = 5) in mat4 a_model;

uniform mat4 u_view_projection;

out vec3 v_normal;
out vec2 v_uv;
out float v_depth;

void main() {
	vec4 world = a_model * vec4(a_position, 1.0);
	v_normal = mat3(a_model) * a_normal;
	v_uv = a_uv;
	gl_Position = u_view_projection * world;
	v_depth = gl_Position.w;
}
