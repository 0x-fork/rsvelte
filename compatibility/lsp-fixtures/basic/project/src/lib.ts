export interface Todo {
	id: number;
	title: string;
	done: boolean;
}

/** Sum the ids of the todos that are still open. */
export function pendingWeight(todos: Todo[]): number {
	return todos.filter((todo) => !todo.done).reduce((total, todo) => total + todo.id, 0);
}

export const emptyTodo: Todo = { id: 0, title: '', done: false };
